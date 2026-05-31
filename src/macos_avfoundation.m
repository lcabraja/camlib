#import <AVFoundation/AVFoundation.h>
#import <AppKit/AppKit.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"

typedef void (*camlib_frame_cb)(void *ctx, const uint8_t *bgra, int width, int height, size_t bytes_per_row);
typedef void (*camlib_status_cb)(void *ctx, const char *message);

static NSArray<AVCaptureDevice *> *camlib_devices(void);
void *camlib_avf_open(const char *unique_id, camlib_frame_cb frame_cb, camlib_status_cb status_cb, void *ctx);
void camlib_avf_close(void *handle);

@interface CamlibCapture : NSObject <AVCaptureVideoDataOutputSampleBufferDelegate>
@property(nonatomic) AVCaptureSession *session;
@property(nonatomic) AVCaptureDeviceInput *input;
@property(nonatomic) AVCaptureVideoDataOutput *output;
@property(nonatomic) dispatch_queue_t queue;
@property(nonatomic) camlib_frame_cb frame_cb;
@property(nonatomic) camlib_status_cb status_cb;
@property(nonatomic) void *ctx;
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
static void picker_status_cb(void *ctx, const char *message);

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

- (void)windowDidResize
{
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
    self.capture = camlib_avf_open(device.uniqueID.UTF8String, picker_frame_cb, picker_status_cb, (__bridge void *)self);
    if (!self.capture) {
        self.status.stringValue = [NSString stringWithFormat:@"Failed to open %@", device.localizedName];
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

static void picker_status_cb(void *ctx, const char *message)
{
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

static NSArray<AVCaptureDevice *> *camlib_devices(void)
{
    NSArray *deviceTypes;
    if (@available(macOS 14, *)) {
        deviceTypes = @[
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeExternal,
            AVCaptureDeviceTypeDeskViewCamera
        ];
    } else if (@available(macOS 13, *)) {
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

static void camlib_status(CamlibCapture *capture, NSString *message)
{
    if (capture.status_cb) {
        capture.status_cb(capture.ctx, message.UTF8String);
    }
}

void *camlib_avf_open(const char *unique_id, camlib_frame_cb frame_cb, camlib_status_cb status_cb, void *ctx)
{
    @autoreleasepool {
        NSString *uuid = unique_id ? [NSString stringWithUTF8String:unique_id] : @"";
        AVCaptureDevice *device = [AVCaptureDevice deviceWithUniqueID:uuid];
        if (!device) {
            return NULL;
        }

        NSError *error = nil;
        CamlibCapture *capture = [[CamlibCapture alloc] init];
        capture.frame_cb = frame_cb;
        capture.status_cb = status_cb;
        capture.ctx = ctx;
        capture.session = [[AVCaptureSession alloc] init];
        capture.queue = dispatch_queue_create("camlib.avfoundation.preview", DISPATCH_QUEUE_SERIAL);

        capture.input = [AVCaptureDeviceInput deviceInputWithDevice:device error:&error];
        if (!capture.input) {
            return NULL;
        }

        capture.output = [[AVCaptureVideoDataOutput alloc] init];
        capture.output.alwaysDiscardsLateVideoFrames = YES;
        capture.output.videoSettings = @{
            (NSString *)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA)
        };
        [capture.output setSampleBufferDelegate:capture queue:capture.queue];

        [capture.session beginConfiguration];
        if ([capture.session canAddInput:capture.input]) {
            [capture.session addInput:capture.input];
        } else {
            [capture.session commitConfiguration];
            return NULL;
        }

        if ([capture.session canAddOutput:capture.output]) {
            [capture.session addOutput:capture.output];
        } else {
            [capture.session commitConfiguration];
            return NULL;
        }

        if ([capture.session canSetSessionPreset:AVCaptureSessionPreset1280x720]) {
            capture.session.sessionPreset = AVCaptureSessionPreset1280x720;
        } else if ([capture.session canSetSessionPreset:AVCaptureSessionPresetHigh]) {
            capture.session.sessionPreset = AVCaptureSessionPresetHigh;
        }
        [capture.session commitConfiguration];

        camlib_status(capture, [NSString stringWithFormat:@"Open: %@ via AVFoundation BGRA output",
                                                          device.localizedName]);
        [capture.session startRunning];
        return (__bridge_retained void *)capture;
    }
}

void camlib_avf_close(void *handle)
{
    if (!handle) {
        return;
    }
    CamlibCapture *capture = (__bridge_transfer CamlibCapture *)handle;
    [capture.session stopRunning];
    [capture.output setSampleBufferDelegate:nil queue:NULL];
    capture.output = nil;
    capture.input = nil;
    capture.session = nil;
    capture.queue = nil;
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
