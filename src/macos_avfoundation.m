#import <AVFoundation/AVFoundation.h>
#import <AppKit/AppKit.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
// Newer device types are weak-linked and only used after a runtime version check below.
#pragma clang diagnostic ignored "-Wunguarded-availability-new"
#pragma clang diagnostic ignored "-Wunguarded-availability"

typedef void (*camlib_frame_cb)(void *ctx, const uint8_t *bgra, int width, int height, size_t bytes_per_row);
typedef void (*camlib_status_cb)(void *ctx, int kind, const char *message);

enum { CAMLIB_STATUS_INFO = 0, CAMLIB_STATUS_ERROR = 1, CAMLIB_STATUS_DISCONNECTED = 2 };

static NSArray<AVCaptureDevice *> *camlib_devices(void);
typedef struct {
    int width;
    int height;
    double frame_rate;
} camlib_format;

void *camlib_avf_open(const char *unique_id, camlib_format *format, camlib_frame_cb frame_cb,
                      camlib_status_cb status_cb, void *ctx, char *error, size_t error_len);
void camlib_avf_close(void *handle);

@interface CamlibCapture : NSObject <AVCaptureVideoDataOutputSampleBufferDelegate>
@property(nonatomic) AVCaptureSession *session;
@property(nonatomic) AVCaptureDeviceInput *input;
@property(nonatomic) AVCaptureVideoDataOutput *output;
@property(nonatomic) dispatch_queue_t queue;
@property(nonatomic) camlib_frame_cb frame_cb;
@property(nonatomic) camlib_status_cb status_cb;
@property(nonatomic) void *ctx;
@property(nonatomic) NSMutableArray<id> *observers;
@end

@interface CamlibPicker : NSObject <NSApplicationDelegate, NSWindowDelegate>
@property(nonatomic) NSWindow *window;
@property(nonatomic) NSPopUpButton *popup;
@property(nonatomic) NSButton *openButton;
@property(nonatomic) NSButton *closeButton;
@property(nonatomic) NSTextField *status;
@property(nonatomic) NSImageView *imageView;
@property(nonatomic) NSArray<AVCaptureDevice *> *devices;
@property(nonatomic) void *capture;
@end

static void picker_frame_cb(void *ctx, const uint8_t *bgra, int width, int height, size_t bytes_per_row);
static void picker_status_cb(void *ctx, int kind, const char *message);

@implementation CamlibPicker
- (void)applicationDidFinishLaunching:(NSNotification *)notification
{
    (void)notification;
    self.devices = camlib_devices();

    self.window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 900, 620)
                                              styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                                                         NSWindowStyleMaskMiniaturizable | NSWindowStyleMaskResizable)
                                                backing:NSBackingStoreBuffered
                                                  defer:NO];
    self.window.title = @"Camera Picker";
    self.window.delegate = self;
    [self.window center];

    NSView *content = self.window.contentView;
    self.popup = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(16, 574, 548, 28) pullsDown:NO];
    for (AVCaptureDevice *device in self.devices) {
        [self.popup addItemWithTitle:device.localizedName];
    }
    [content addSubview:self.popup];

    self.openButton = [NSButton buttonWithTitle:@"Open" target:self action:@selector(openCamera:)];
    self.openButton.frame = NSMakeRect(576, 574, 96, 28);
    [content addSubview:self.openButton];

    self.closeButton = [NSButton buttonWithTitle:@"Close" target:self action:@selector(closeCamera:)];
    self.closeButton.frame = NSMakeRect(680, 574, 96, 28);
    [content addSubview:self.closeButton];

    self.status = [[NSTextField alloc] initWithFrame:NSMakeRect(16, 540, 868, 24)];
    self.status.editable = NO;
    self.status.bezeled = NO;
    self.status.drawsBackground = NO;
    self.status.stringValue = [NSString stringWithFormat:@"%lu camera(s) found", (unsigned long)self.devices.count];
    [content addSubview:self.status];

    self.imageView = [[NSImageView alloc] initWithFrame:NSMakeRect(16, 16, 868, 512)];
    self.imageView.imageScaling = NSImageScaleProportionallyUpOrDown;
    self.imageView.wantsLayer = YES;
    self.imageView.layer.backgroundColor = NSColor.blackColor.CGColor;
    [content addSubview:self.imageView];

    self.window.contentMinSize = NSMakeSize(520, 360);
    [self.window makeKeyAndOrderFront:nil];
    [NSApp activateIgnoringOtherApps:YES];
}

- (void)windowDidResize:(NSNotification *)notification
{
    (void)notification;
    NSView *content = self.window.contentView;
    CGFloat width = content.bounds.size.width;
    CGFloat height = content.bounds.size.height;
    self.popup.frame = NSMakeRect(16, height - 46, MAX(180, width - 352), 28);
    self.openButton.frame = NSMakeRect(width - 324, height - 46, 96, 28);
    self.closeButton.frame = NSMakeRect(width - 220, height - 46, 96, 28);
    self.status.frame = NSMakeRect(16, height - 80, width - 32, 24);
    self.imageView.frame = NSMakeRect(16, 16, width - 32, height - 108);
}

- (void)openCamera:(id)sender
{
    (void)sender;
    [self closeCamera:nil];
    NSInteger index = self.popup.indexOfSelectedItem;
    if (index < 0 || index >= (NSInteger)self.devices.count) {
        self.status.stringValue = @"No camera selected";
        return;
    }

    AVCaptureDevice *device = self.devices[(NSUInteger)index];
    char error[512] = {0};
    camlib_format format = {1280, 720, 30};
    self.capture = camlib_avf_open(device.uniqueID.UTF8String, &format, picker_frame_cb, picker_status_cb,
                                   (__bridge void *)self, error, sizeof error);
    if (!self.capture) {
        self.status.stringValue = [NSString stringWithFormat:@"Failed to open %@: %s", device.localizedName, error];
    }
}

- (void)closeCamera:(id)sender
{
    (void)sender;
    if (self.capture) {
        camlib_avf_close(self.capture);
        self.capture = NULL;
    }
    self.imageView.image = nil;
    self.status.stringValue = @"Closed";
}

- (void)applicationWillTerminate:(NSNotification *)notification
{
    (void)notification;
    [self closeCamera:nil];
}
@end

static void picker_frame_cb(void *ctx, const uint8_t *bgra, int width, int height, size_t bytes_per_row)
{
    if (!ctx || !bgra || width <= 0 || height <= 0) {
        return;
    }

    size_t byteCount = bytes_per_row * (size_t)height;
    CFDataRef data = CFDataCreate(kCFAllocatorDefault, bgra, (CFIndex)byteCount);
    CGDataProviderRef provider = CGDataProviderCreateWithCFData(data);
    CGColorSpaceRef colorSpace = CGColorSpaceCreateDeviceRGB();
    CGImageRef image = CGImageCreate((size_t)width, (size_t)height, 8, 32, bytes_per_row, colorSpace,
                                     kCGBitmapByteOrder32Little | kCGImageAlphaFirst, provider, NULL, false,
                                     kCGRenderingIntentDefault);
    NSImage *nsImage = [[NSImage alloc] initWithCGImage:image size:NSMakeSize(width, height)];

    CamlibPicker *picker = (__bridge CamlibPicker *)ctx;
    dispatch_async(dispatch_get_main_queue(), ^{
        picker.imageView.image = nsImage;
    });

    CGImageRelease(image);
    CGColorSpaceRelease(colorSpace);
    CGDataProviderRelease(provider);
    CFRelease(data);
}

static void picker_status_cb(void *ctx, int kind, const char *message)
{
    (void)kind;
    if (!ctx || !message) {
        return;
    }
    CamlibPicker *picker = (__bridge CamlibPicker *)ctx;
    NSString *status = [NSString stringWithUTF8String:message];
    dispatch_async(dispatch_get_main_queue(), ^{
        picker.status.stringValue = status ?: @"";
    });
}

@implementation CamlibCapture
- (void)captureOutput:(AVCaptureOutput *)output
    didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
           fromConnection:(AVCaptureConnection *)connection
{
    (void)output;
    (void)connection;
    CVImageBufferRef imageBuffer = CMSampleBufferGetImageBuffer(sampleBuffer);
    if (!imageBuffer || !self.frame_cb) {
        return;
    }

    CVPixelBufferLockBaseAddress(imageBuffer, kCVPixelBufferLock_ReadOnly);
    const uint8_t *base = CVPixelBufferGetBaseAddress(imageBuffer);
    int width = (int)CVPixelBufferGetWidth(imageBuffer);
    int height = (int)CVPixelBufferGetHeight(imageBuffer);
    size_t bytesPerRow = CVPixelBufferGetBytesPerRow(imageBuffer);
    if (base && width > 0 && height > 0) {
        self.frame_cb(self.ctx, base, width, height, bytesPerRow);
    }
    CVPixelBufferUnlockBaseAddress(imageBuffer, kCVPixelBufferLock_ReadOnly);
}
@end

// `@available` would need ___isPlatformVersionAtLeast from compiler-rt, which Rust's linker does
// not provide in release builds; NSProcessInfo answers the same question.
static BOOL camlib_at_least(NSInteger major)
{
    NSOperatingSystemVersion version = {major, 0, 0};
    return [NSProcessInfo.processInfo isOperatingSystemAtLeastVersion:version];
}

static NSArray<AVCaptureDevice *> *camlib_devices(void)
{
    NSArray *deviceTypes;
    if (camlib_at_least(14)) {
        deviceTypes = @[
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeExternal,
            AVCaptureDeviceTypeContinuityCamera,
            AVCaptureDeviceTypeDeskViewCamera
        ];
    } else if (camlib_at_least(13)) {
        deviceTypes = @[
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeExternalUnknown,
            AVCaptureDeviceTypeDeskViewCamera
        ];
    } else {
        deviceTypes = @[AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureDeviceTypeExternalUnknown];
    }

    AVCaptureDeviceDiscoverySession *videoSession =
        [AVCaptureDeviceDiscoverySession discoverySessionWithDeviceTypes:deviceTypes
                                                               mediaType:AVMediaTypeVideo
                                                                position:AVCaptureDevicePositionUnspecified];
    AVCaptureDeviceDiscoverySession *muxedSession =
        [AVCaptureDeviceDiscoverySession discoverySessionWithDeviceTypes:deviceTypes
                                                               mediaType:AVMediaTypeMuxed
                                                                position:AVCaptureDevicePositionUnspecified];

    NSMutableArray<AVCaptureDevice *> *devices = [NSMutableArray array];
    NSMutableSet<NSString *> *ids = [NSMutableSet set];
    for (AVCaptureDevice *device in videoSession.devices) {
        if (![ids containsObject:device.uniqueID]) {
            [devices addObject:device];
            [ids addObject:device.uniqueID];
        }
    }
    for (AVCaptureDevice *device in muxedSession.devices) {
        if (![ids containsObject:device.uniqueID]) {
            [devices addObject:device];
            [ids addObject:device.uniqueID];
        }
    }
    return devices;
}

static NSString *camlib_clean(NSString *value)
{
    NSString *clean = [value stringByReplacingOccurrencesOfString:@"\t" withString:@" "];
    clean = [clean stringByReplacingOccurrencesOfString:@"\n" withString:@" "];
    return clean ?: @"";
}

size_t camlib_avf_list(char *buffer, size_t buffer_len)
{
    NSMutableString *result = [NSMutableString string];
    for (AVCaptureDevice *device in camlib_devices()) {
        [result appendFormat:@"%@\t%@\t%@\n",
                             camlib_clean(device.uniqueID),
                             camlib_clean(device.localizedName),
                             camlib_clean(device.modelID ?: @"")];
    }

    NSData *data = [result dataUsingEncoding:NSUTF8StringEncoding];
    size_t required = data.length + 1;
    if (buffer && buffer_len > 0) {
        size_t copyLen = MIN(data.length, buffer_len - 1);
        memcpy(buffer, data.bytes, copyLen);
        buffer[copyLen] = '\0';
    }
    return required;
}

static void camlib_status(CamlibCapture *capture, int kind, NSString *message)
{
    if (capture.status_cb) {
        capture.status_cb(capture.ctx, kind, message.UTF8String);
    }
}

static void camlib_set_error(char *error, size_t error_len, NSString *message)
{
    if (!error || error_len == 0) {
        return;
    }
    const char *utf8 = message.UTF8String ?: "unknown AVFoundation error";
    strlcpy(error, utf8, error_len);
}

int camlib_avf_authorization(void)
{
    switch ([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo]) {
    case AVAuthorizationStatusAuthorized:
        return 0;
    case AVAuthorizationStatusNotDetermined:
        return 1;
    case AVAuthorizationStatusDenied:
        return 2;
    case AVAuthorizationStatusRestricted:
        return 3;
    }
    return 2;
}

int camlib_avf_request_access(void)
{
    if ([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo] != AVAuthorizationStatusNotDetermined) {
        return camlib_avf_authorization() == 0;
    }
    // The completion handler runs on an arbitrary queue, so waiting here is safe off the main thread.
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    __block BOOL granted = NO;
    [AVCaptureDevice requestAccessForMediaType:AVMediaTypeVideo
                             completionHandler:^(BOOL ok) {
                                 granted = ok;
                                 dispatch_semaphore_signal(done);
                             }];
    dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
    return granted ? 1 : 0;
}

// Choose the advertised mode nearest the request, preferring the requested size over frame rate
// and avoiding very slow modes. Mirrors OBS picking an explicit device format instead of a preset.
static AVCaptureDeviceFormat *camlib_select_format(AVCaptureDevice *device, camlib_format request,
                                                   AVFrameRateRange **bestRange, double *bestRate)
{
    AVCaptureDeviceFormat *best = nil;
    double bestCost = INFINITY;
    for (AVCaptureDeviceFormat *format in device.formats) {
        CMVideoDimensions dims = CMVideoFormatDescriptionGetDimensions(format.formatDescription);
        if (dims.width <= 0 || dims.height <= 0) {
            continue;
        }
        double dw = (double)dims.width / request.width - 1.0;
        double dh = (double)dims.height / request.height - 1.0;
        for (AVFrameRateRange *range in format.videoSupportedFrameRateRanges) {
            double rate = MIN(MAX(request.frame_rate, range.minFrameRate), range.maxFrameRate);
            double df = fabs(rate - request.frame_rate) / request.frame_rate;
            double cost = dw * dw + dh * dh + df * 0.25 + (rate < 15 ? 1.0 : 0.0);
            if (cost < bestCost) {
                bestCost = cost;
                best = format;
                *bestRange = range;
                *bestRate = rate;
            }
        }
    }
    return best;
}

static CMTime camlib_frame_duration(AVFrameRateRange *range, double rate)
{
    // Exact range bounds avoid rejecting NTSC-style rates such as 29.97.
    if (fabs(rate - range.maxFrameRate) < 0.001) {
        return range.minFrameDuration;
    }
    if (fabs(rate - range.minFrameRate) < 0.001) {
        return range.maxFrameDuration;
    }
    return CMTimeMake(1000, (int32_t)llround(rate * 1000.0));
}

void *camlib_avf_open(const char *unique_id, camlib_format *format, camlib_frame_cb frame_cb,
                      camlib_status_cb status_cb, void *ctx, char *error, size_t error_len)
{
    @autoreleasepool {
        if (camlib_avf_authorization() != 0) {
            camlib_set_error(error, error_len, @"Camera access is not authorized");
            return NULL;
        }

        NSString *uuid = unique_id ? [NSString stringWithUTF8String:unique_id] : @"";
        AVCaptureDevice *device = [AVCaptureDevice deviceWithUniqueID:uuid];
        if (!device) {
            camlib_set_error(error, error_len, [NSString stringWithFormat:@"camera '%@' was not found", uuid]);
            return NULL;
        }

        NSError *inputError = nil;
        CamlibCapture *capture = [[CamlibCapture alloc] init];
        capture.frame_cb = frame_cb;
        capture.status_cb = status_cb;
        capture.ctx = ctx;
        capture.observers = [NSMutableArray array];
        capture.session = [[AVCaptureSession alloc] init];
        capture.queue = dispatch_queue_create("camlib.avfoundation.capture", DISPATCH_QUEUE_SERIAL);

        capture.input = [AVCaptureDeviceInput deviceInputWithDevice:device error:&inputError];
        if (!capture.input) {
            camlib_set_error(error, error_len,
                             inputError.localizedDescription ?: @"AVFoundation could not create a device input");
            return NULL;
        }

        capture.output = [[AVCaptureVideoDataOutput alloc] init];
        capture.output.alwaysDiscardsLateVideoFrames = YES;
        capture.output.videoSettings = @{
            (NSString *)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA)
        };
        [capture.output setSampleBufferDelegate:capture queue:capture.queue];

        [capture.session beginConfiguration];
        if (![capture.session canAddInput:capture.input]) {
            [capture.session commitConfiguration];
            camlib_set_error(error, error_len, @"AVFoundation session rejected the camera input");
            return NULL;
        }
        [capture.session addInput:capture.input];

        if (![capture.session canAddOutput:capture.output]) {
            [capture.session commitConfiguration];
            camlib_set_error(error, error_len, @"AVFoundation session rejected the BGRA video output");
            return NULL;
        }
        [capture.session addOutput:capture.output];

        if ([capture.session canSetSessionPreset:AVCaptureSessionPreset1280x720]) {
            capture.session.sessionPreset = AVCaptureSessionPreset1280x720;
        }
        [capture.session commitConfiguration];

        // macOS sessions apply their preset on commit, so pick the device format afterwards and hold the
        // lock through startRunning; presets alone leave many devices at 640x480.
        BOOL locked = NO;
        AVFrameRateRange *range = nil;
        double rate = 0;
        AVCaptureDeviceFormat *selected = nil;
        if (format && format->width > 0 && format->height > 0 && format->frame_rate > 0) {
            selected = camlib_select_format(device, *format, &range, &rate);
        }
        NSError *lockError = nil;
        if (selected && [device lockForConfiguration:&lockError]) {
            locked = YES;
            @try {
                device.activeFormat = selected;
                CMTime duration = camlib_frame_duration(range, rate);
                device.activeVideoMinFrameDuration = duration;
                device.activeVideoMaxFrameDuration = duration;
            } @catch (NSException *exception) {
                // Some virtual cameras advertise rates they refuse; keep their default rate.
                (void)exception;
            }
        }
        NSString *modeNote = @"";
        if (selected && !locked) {
            // Another app owns the device configuration; share the camera in its current mode.
            modeNote = [NSString stringWithFormat:@" (current mode kept: %@)",
                                                  lockError.localizedDescription ?: @"device is locked"];
        }

        // Observers hold the capture weakly so they never outlive camlib_avf_close.
        __weak CamlibCapture *weakCapture = capture;
        NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
        [capture.observers addObject:[center addObserverForName:AVCaptureSessionRuntimeErrorNotification
                                                         object:capture.session
                                                          queue:nil
                                                     usingBlock:^(NSNotification *note) {
                                                         NSError *runtimeError = note.userInfo[AVCaptureSessionErrorKey];
                                                         CamlibCapture *strong = weakCapture;
                                                         NSString *message = runtimeError.localizedDescription
                                                                                 ?: @"AVFoundation session failed";
                                                         if (strong) {
                                                             // Serialize with frames so close can drain it.
                                                             dispatch_async(strong.queue, ^{
                                                                 camlib_status(strong, CAMLIB_STATUS_ERROR, message);
                                                             });
                                                         }
                                                     }]];
        [capture.observers addObject:[center addObserverForName:AVCaptureDeviceWasDisconnectedNotification
                                                         object:device
                                                          queue:nil
                                                     usingBlock:^(NSNotification *note) {
                                                         (void)note;
                                                         CamlibCapture *strong = weakCapture;
                                                         if (strong) {
                                                             dispatch_async(strong.queue, ^{
                                                                 camlib_status(strong, CAMLIB_STATUS_DISCONNECTED,
                                                                               @"Camera was disconnected");
                                                             });
                                                         }
                                                     }]];

        camlib_status(capture, CAMLIB_STATUS_INFO,
                      [NSString stringWithFormat:@"Open: %@ via AVFoundation BGRA output%@", device.localizedName,
                                                 modeNote]);
        [capture.session startRunning];
        // Holding the lock through startRunning stops the session from reapplying its preset.
        if (locked) {
            [device unlockForConfiguration];
        }
        if (format) {
            CMVideoDimensions dims = CMVideoFormatDescriptionGetDimensions(device.activeFormat.formatDescription);
            CMTime duration = device.activeVideoMinFrameDuration;
            format->width = dims.width;
            format->height = dims.height;
            format->frame_rate =
                CMTIME_IS_VALID(duration) && duration.value > 0 ? (double)duration.timescale / duration.value : 0;
        }
        if (!capture.session.isRunning) {
            camlib_avf_close((__bridge_retained void *)capture);
            camlib_set_error(error, error_len, @"AVFoundation session did not start");
            return NULL;
        }
        return (__bridge_retained void *)capture;
    }
}

void camlib_avf_close(void *handle)
{
    if (!handle) {
        return;
    }
    @autoreleasepool {
        CamlibCapture *capture = (__bridge_transfer CamlibCapture *)handle;
        for (id observer in capture.observers) {
            [NSNotificationCenter.defaultCenter removeObserver:observer];
        }
        [capture.observers removeAllObjects];
        [capture.session stopRunning];
        [capture.output setSampleBufferDelegate:nil queue:NULL];
        // Drain any delegate callback already running so the caller may free ctx after return.
        dispatch_sync(capture.queue, ^{
            capture.frame_cb = NULL;
            capture.status_cb = NULL;
            capture.ctx = NULL;
        });
        capture.output = nil;
        capture.input = nil;
        capture.session = nil;
    }
}

void camlib_avf_run_picker(void)
{
    @autoreleasepool {
        NSApplication *app = [NSApplication sharedApplication];
        app.activationPolicy = NSApplicationActivationPolicyRegular;
        CamlibPicker *delegate = [[CamlibPicker alloc] init];
        app.delegate = delegate;
        [app run];
    }
}

#pragma clang diagnostic pop
