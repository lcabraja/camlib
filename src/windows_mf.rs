//! Media Foundation capture. OBS uses DirectShow on Windows; Media Foundation is the supported
//! successor and its source reader decodes MJPEG and converts YUV for us, so frames always
//! arrive as RGB32. COM interfaces are called through their vtables by index, in the order of
//! the Windows SDK (and mingw-w64) headers, keeping camlib free of crate dependencies.

use crate::{
    CameraBackend, CameraDevice, CameraError, CameraFormat, PixelFormat, Result,
    convert::{self, Layout, RawFrame, Rgb},
    modes::{self, Mode},
    threaded::{Interrupt, Stream, StreamError, ThreadedCamera},
};
use std::{ffi::c_void, ptr, time::Duration};

type HResult = i32;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid(u32, u16, u16, [u8; 8]);

const fn media_subtype(fourcc: u32) -> Guid {
    Guid(
        fourcc,
        0x0000,
        0x0010,
        [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71],
    )
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}

const MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE: Guid = Guid(
    0xc60ac5fe,
    0x252a,
    0x478f,
    [0xa0, 0xef, 0xbc, 0x8f, 0xa5, 0xf7, 0xca, 0xd3],
);
const MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID: Guid = Guid(
    0x8ac3587a,
    0x4ae7,
    0x42d8,
    [0x99, 0xe0, 0x0a, 0x60, 0x13, 0xee, 0xf9, 0x0f],
);
const MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME: Guid = Guid(
    0x60d0e559,
    0x52f8,
    0x4fa2,
    [0xbb, 0xce, 0xac, 0xdb, 0x34, 0xa8, 0xec, 0x01],
);
const MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK: Guid = Guid(
    0x58f0aad8,
    0x22bf,
    0x4f8a,
    [0xbb, 0x3d, 0xd2, 0xc4, 0x97, 0x8c, 0x6e, 0x2f],
);
const MF_MT_MAJOR_TYPE: Guid = Guid(
    0x48eba18e,
    0xf8c9,
    0x4687,
    [0xbf, 0x11, 0x0a, 0x74, 0xc9, 0xf9, 0x6a, 0x8f],
);
const MF_MT_SUBTYPE: Guid = Guid(
    0xf7e34c9a,
    0x42e8,
    0x4714,
    [0xb7, 0x4b, 0xcb, 0x29, 0xd7, 0x2c, 0x35, 0xe5],
);
const MF_MT_FRAME_SIZE: Guid = Guid(
    0x1652c33d,
    0xd6b2,
    0x4012,
    [0xb8, 0x34, 0x72, 0x03, 0x08, 0x49, 0xa3, 0x7d],
);
const MF_MT_FRAME_RATE: Guid = Guid(
    0xc459a2e8,
    0x3d2c,
    0x4e44,
    [0xb1, 0x32, 0xfe, 0xe5, 0x15, 0x6c, 0x7b, 0xb0],
);
const MF_MT_DEFAULT_STRIDE: Guid = Guid(
    0x644b4e48,
    0x1e02,
    0x4516,
    [0xb0, 0xeb, 0xc0, 0x1c, 0xa9, 0xd4, 0x9a, 0xc6],
);
const MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING: Guid = Guid(
    0x0f81da2c,
    0xb537,
    0x4672,
    [0xa8, 0xb2, 0xa6, 0x81, 0xb1, 0x73, 0x07, 0xa3],
);
const IID_IMF2DBUFFER: Guid = Guid(
    0x7dc9d5f9,
    0x9ed9,
    0x44ec,
    [0x9b, 0xbf, 0x06, 0x00, 0xbb, 0x58, 0x9f, 0xbb],
);
const IID_IMFMEDIASOURCE: Guid = Guid(
    0x279a808d,
    0xaec7,
    0x40c8,
    [0x9c, 0x6b, 0xa6, 0xb4, 0x92, 0xc7, 0x8a, 0x66],
);
const MF_MEDIATYPE_VIDEO: Guid = media_subtype(fourcc(b"vids"));
/// `D3DFMT_X8R8G8B8`: B, G, R, X bytes in memory.
const MF_VIDEOFORMAT_RGB32: Guid = media_subtype(22);
const MF_VIDEOFORMAT_RGB24: Guid = media_subtype(20);

const MF_VERSION: u32 = 0x0002 << 16 | 0x0070;
const MFSTARTUP_LITE: u32 = 1;
const COINIT_MULTITHREADED: u32 = 0;
const RPC_E_CHANGED_MODE: HResult = 0x8001_0106_u32 as HResult;
const MF_E_NO_MORE_TYPES: HResult = 0xc00d_36b9_u32 as HResult;
const FIRST_VIDEO_STREAM: u32 = 0xffff_fffc;
const ALL_STREAMS: u32 = 0xffff_fffe;
const READERF_ERROR: u32 = 0x1;
const READERF_ENDOFSTREAM: u32 = 0x2;
const READERF_CURRENTMEDIATYPECHANGED: u32 = 0x20;

// Vtable slots. IUnknown is 0-2; IMFAttributes adds 3-32.
const RELEASE: usize = 2;
const QUERY_INTERFACE: usize = 0;
const ATTR_GET_UINT32: usize = 7;
const ATTR_GET_UINT64: usize = 8;
const ATTR_GET_GUID: usize = 10;
const ATTR_GET_ALLOCATED_STRING: usize = 13;
const ATTR_SET_UINT32: usize = 21;
const ATTR_SET_UINT64: usize = 22;
const ATTR_SET_GUID: usize = 24;
const ATTR_SET_STRING: usize = 25;
const SAMPLE_CONVERT_TO_CONTIGUOUS_BUFFER: usize = 41;
const BUFFER_LOCK: usize = 3;
const BUFFER_UNLOCK: usize = 4;
const BUFFER_2D_LOCK: usize = 3;
const BUFFER_2D_UNLOCK: usize = 4;
const SOURCE_SHUTDOWN: usize = 12;
const READER_SET_STREAM_SELECTION: usize = 4;
const READER_GET_NATIVE_MEDIA_TYPE: usize = 5;
const READER_GET_CURRENT_MEDIA_TYPE: usize = 6;
const READER_SET_CURRENT_MEDIA_TYPE: usize = 7;
const READER_READ_SAMPLE: usize = 9;

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> HResult;
    fn CoUninitialize();
    fn CoTaskMemFree(memory: *mut c_void);
}

#[link(name = "mfplat")]
unsafe extern "system" {
    fn MFStartup(version: u32, flags: u32) -> HResult;
    fn MFShutdown() -> HResult;
    fn MFCreateAttributes(attributes: *mut *mut c_void, initial_size: u32) -> HResult;
    fn MFCreateMediaType(media_type: *mut *mut c_void) -> HResult;
}

#[link(name = "mf")]
unsafe extern "system" {
    fn MFEnumDeviceSources(
        attributes: *mut c_void,
        sources: *mut *mut *mut c_void,
        count: *mut u32,
    ) -> HResult;
    fn MFCreateDeviceSource(attributes: *mut c_void, source: *mut *mut c_void) -> HResult;
}

#[link(name = "mfreadwrite")]
unsafe extern "system" {
    fn MFCreateSourceReaderFromMediaSource(
        source: *mut c_void,
        attributes: *mut c_void,
        reader: *mut *mut c_void,
    ) -> HResult;
    #[cfg(feature = "test-sources")]
    fn MFCreateSourceReaderFromURL(
        url: *const u16,
        attributes: *mut c_void,
        reader: *mut *mut c_void,
    ) -> HResult;
}

fn check(hr: HResult, what: &str) -> Result<()> {
    if hr < 0 {
        Err(CameraError::Native(format!(
            "{what} failed (HRESULT 0x{:08x})",
            hr as u32
        )))
    } else {
        Ok(())
    }
}

/// An owned COM interface pointer, released on drop.
struct Com(*mut c_void);

// Media Foundation objects created in the multithreaded apartment are free-threaded.
unsafe impl Send for Com {}

impl Com {
    fn from_out(raw: *mut c_void, what: &str) -> Result<Self> {
        if raw.is_null() {
            Err(CameraError::Native(format!("{what} returned no object")))
        } else {
            Ok(Self(raw))
        }
    }

    /// The function pointer in vtable `slot`.
    unsafe fn slot(&self, slot: usize) -> *const c_void {
        unsafe { *(*(self.0 as *const *const *const c_void)).add(slot) }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            let release: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute(self.slot(RELEASE));
            release(self.0);
        }
    }
}

/// Call vtable `slot` on `com` with the given argument types and values.
macro_rules! call {
    ($com:expr, $slot:expr $(, $arg:expr => $ty:ty)* $(,)?) => {{
        let com: &Com = &$com;
        let method: unsafe extern "system" fn(*mut c_void $(, $ty)*) -> HResult =
            unsafe { std::mem::transmute(com.slot($slot)) };
        unsafe { method(com.0 $(, $arg)*) }
    }};
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn get_u64(attributes: &Com, key: &Guid) -> Option<u64> {
    let mut value = 0u64;
    (call!(attributes, ATTR_GET_UINT64, key => *const Guid, &mut value => *mut u64) >= 0)
        .then_some(value)
}

fn get_guid(attributes: &Com, key: &Guid) -> Option<Guid> {
    let mut value = Guid(0, 0, 0, [0; 8]);
    (call!(attributes, ATTR_GET_GUID, key => *const Guid, &mut value => *mut Guid) >= 0)
        .then_some(value)
}

fn get_string(attributes: &Com, key: &Guid) -> Option<String> {
    let mut text: *mut u16 = ptr::null_mut();
    let mut length = 0u32;
    let hr = call!(attributes, ATTR_GET_ALLOCATED_STRING,
        key => *const Guid, &mut text => *mut *mut u16, &mut length => *mut u32);
    if hr < 0 || text.is_null() {
        return None;
    }
    let value =
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length as usize) });
    unsafe { CoTaskMemFree(text.cast()) };
    Some(value)
}

fn split(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

/// COM and Media Foundation initialized for the current thread, undone on drop.
struct Runtime {
    uninitialize_com: bool,
}

impl Runtime {
    fn start() -> Result<Self> {
        let hr = unsafe { CoInitializeEx(ptr::null_mut(), COINIT_MULTITHREADED) };
        // A thread already in another apartment can still use free-threaded MF objects.
        if hr < 0 && hr != RPC_E_CHANGED_MODE {
            check(hr, "CoInitializeEx")?;
        }
        let runtime = Self {
            uninitialize_com: hr >= 0,
        };
        check(
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) },
            "MFStartup",
        )?;
        Ok(runtime)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            MFShutdown();
            if self.uninitialize_com {
                CoUninitialize();
            }
        }
    }
}

fn video_capture_attributes(capacity: u32) -> Result<Com> {
    let mut raw = ptr::null_mut();
    check(
        unsafe { MFCreateAttributes(&mut raw, capacity) },
        "MFCreateAttributes",
    )?;
    let attributes = Com::from_out(raw, "MFCreateAttributes")?;
    check(
        call!(attributes, ATTR_SET_GUID,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE => *const Guid,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID => *const Guid),
        "selecting video capture devices",
    )?;
    Ok(attributes)
}

fn enumerate() -> Result<Vec<CameraDevice>> {
    let _runtime = Runtime::start()?;
    let attributes = video_capture_attributes(1)?;
    let mut sources: *mut *mut c_void = ptr::null_mut();
    let mut count = 0u32;
    check(
        unsafe { MFEnumDeviceSources(attributes.0, &mut sources, &mut count) },
        "MFEnumDeviceSources",
    )?;
    let mut devices = Vec::new();
    if sources.is_null() {
        return Ok(devices);
    }
    // Take ownership of every activation object before reading any, so all are released.
    let activations: Vec<Com> = (0..count as usize)
        .filter_map(|i| {
            let raw = unsafe { *sources.add(i) };
            (!raw.is_null()).then_some(Com(raw))
        })
        .collect();
    unsafe { CoTaskMemFree(sources.cast()) };
    for activation in &activations {
        let Some(id) = get_string(
            activation,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
        ) else {
            continue;
        };
        let label = get_string(activation, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
            .unwrap_or_else(|| "Camera".into());
        devices.push(CameraDevice {
            id,
            label,
            backend: CameraBackend::MediaFoundation,
            description: String::new(),
        });
    }
    Ok(devices)
}

pub fn list_cameras() -> Result<Vec<CameraDevice>> {
    // Enumerate on a fresh MTA thread so the caller's COM apartment never matters.
    std::thread::spawn(enumerate)
        .join()
        .map_err(|_| CameraError::Native("camera enumeration panicked".into()))?
}

fn layout(subtype: &Guid) -> Option<Layout> {
    if subtype.1 != 0 || subtype.2 != 0x0010 {
        return None;
    }
    Some(match &subtype.0.to_le_bytes() {
        b"YUY2" => Layout::Yuyv,
        b"UYVY" => Layout::Uyvy,
        b"YVYU" => Layout::Yvyu,
        b"NV12" => Layout::Nv12,
        b"I420" | b"IYUV" => Layout::I420,
        b"YV12" => Layout::Yv12,
        b"MJPG" => Layout::Mjpeg,
        _ if *subtype == MF_VIDEOFORMAT_RGB32 => Layout::Bgrx,
        _ if *subtype == MF_VIDEOFORMAT_RGB24 => Layout::Bgr24,
        _ => return None,
    })
}

struct MfStream {
    // Field order is drop order: the reader and source go before Media Foundation shuts down.
    reader: Com,
    source: Option<Com>,
    width: usize,
    height: usize,
    /// Bytes per row; negative for bottom-up rows.
    default_stride: i32,
    _runtime: Runtime,
}

impl MfStream {
    fn open(
        source: Option<Com>,
        reader: Com,
        runtime: Runtime,
        preferred: CameraFormat,
    ) -> Result<(Self, CameraFormat)> {
        let mut candidates = Vec::new();
        for index in 0.. {
            let mut raw = ptr::null_mut();
            let hr = call!(reader, READER_GET_NATIVE_MEDIA_TYPE,
                FIRST_VIDEO_STREAM => u32, index => u32, &mut raw => *mut *mut c_void);
            if hr == MF_E_NO_MORE_TYPES {
                break;
            }
            check(hr, "reading camera formats")?;
            let media_type = Com::from_out(raw, "GetNativeMediaType")?;
            let (Some(subtype), Some(size)) = (
                get_guid(&media_type, &MF_MT_SUBTYPE),
                get_u64(&media_type, &MF_MT_FRAME_SIZE),
            ) else {
                continue;
            };
            // The reader decodes compressed types (MJPEG, H.264) itself; prefer raw ones.
            let decode_cost = layout(&subtype).map_or(modes::COMPRESSED, Layout::decode_cost);
            let (width, height) = split(size);
            let frame_rate = get_u64(&media_type, &MF_MT_FRAME_RATE)
                .map(split)
                .filter(|&(_, den)| den > 0)
                .map_or(30.0, |(num, den)| f64::from(num) / f64::from(den));
            candidates.push((
                Mode {
                    width,
                    height,
                    frame_rate,
                    decode_cost,
                },
                media_type,
            ));
        }
        let (mode, native) = modes::best(candidates, preferred).ok_or_else(|| {
            CameraError::Native("camera offers no format Media Foundation can convert".into())
        })?;
        check(
            call!(reader, READER_SET_CURRENT_MEDIA_TYPE,
                FIRST_VIDEO_STREAM => u32, ptr::null_mut::<u32>() => *mut u32, native.0 => *mut c_void),
            "selecting the camera format",
        )?;

        // Ask the reader to decode and convert that format to RGB32.
        let mut raw = ptr::null_mut();
        check(unsafe { MFCreateMediaType(&mut raw) }, "MFCreateMediaType")?;
        let output = Com::from_out(raw, "MFCreateMediaType")?;
        check(
            call!(output, ATTR_SET_GUID, &MF_MT_MAJOR_TYPE => *const Guid, &MF_MEDIATYPE_VIDEO => *const Guid),
            "building the output format",
        )?;
        check(
            call!(output, ATTR_SET_GUID, &MF_MT_SUBTYPE => *const Guid, &MF_VIDEOFORMAT_RGB32 => *const Guid),
            "building the output format",
        )?;
        if let Some(size) = get_u64(&native, &MF_MT_FRAME_SIZE) {
            call!(output, ATTR_SET_UINT64, &MF_MT_FRAME_SIZE => *const Guid, size => u64);
            // Ask for top-down rows explicitly; without a stride, RGB may arrive bottom-up
            // (DirectShow convention, and what Wine does).
            let stride = split(size).0.saturating_mul(4);
            call!(output, ATTR_SET_UINT32, &MF_MT_DEFAULT_STRIDE => *const Guid, stride => u32);
        }
        if let Some(rate) = get_u64(&native, &MF_MT_FRAME_RATE) {
            call!(output, ATTR_SET_UINT64, &MF_MT_FRAME_RATE => *const Guid, rate => u64);
        }
        check(
            call!(reader, READER_SET_CURRENT_MEDIA_TYPE,
                FIRST_VIDEO_STREAM => u32, ptr::null_mut::<u32>() => *mut u32, output.0 => *mut c_void),
            "converting the camera format to RGB32",
        )?;
        call!(reader, READER_SET_STREAM_SELECTION, ALL_STREAMS => u32, 0 => i32);
        check(
            call!(reader, READER_SET_STREAM_SELECTION, FIRST_VIDEO_STREAM => u32, 1 => i32),
            "selecting the video stream",
        )?;

        let mut stream = Self {
            reader,
            source,
            width: mode.width as usize,
            height: mode.height as usize,
            default_stride: 0,
            _runtime: runtime,
        };
        stream.refresh_type()?;
        let format = CameraFormat::new(
            stream.width as u32,
            stream.height as u32,
            mode.frame_rate.round() as u32,
            PixelFormat::Rgb,
        );
        Ok((stream, format))
    }

    /// Re-read the delivered size and stride after the reader (re)negotiates its output.
    fn refresh_type(&mut self) -> Result<()> {
        let mut raw = ptr::null_mut();
        check(
            call!(self.reader, READER_GET_CURRENT_MEDIA_TYPE, FIRST_VIDEO_STREAM => u32, &mut raw => *mut *mut c_void),
            "reading the output format",
        )?;
        let current = Com::from_out(raw, "GetCurrentMediaType")?;
        if let Some(size) = get_u64(&current, &MF_MT_FRAME_SIZE) {
            let (width, height) = split(size);
            self.width = width as usize;
            self.height = height as usize;
        }
        let mut stride = 0u32;
        self.default_stride = if call!(current, ATTR_GET_UINT32,
            &MF_MT_DEFAULT_STRIDE => *const Guid, &mut stride => *mut u32)
            >= 0
        {
            stride as i32
        } else {
            (self.width * 4) as i32
        };
        Ok(())
    }

    fn convert(&self, sample: &Com) -> std::result::Result<Rgb, StreamError> {
        let mut raw = ptr::null_mut();
        let hr = call!(sample, SAMPLE_CONVERT_TO_CONTIGUOUS_BUFFER, &mut raw => *mut *mut c_void);
        if hr < 0 || raw.is_null() {
            return Err(StreamError::Frame("sample has no buffer".into()));
        }
        let buffer = Com(raw);
        let (height, width) = (self.height, self.width);
        let convert_rows = |scan0: *const u8, pitch: i32, available: Option<usize>| {
            let stride = pitch.unsigned_abs() as usize;
            let len = stride * height;
            if available.is_some_and(|available| available < len) || stride < width * 4 {
                return Err(StreamError::Frame("RGB32 sample is too small".into()));
            }
            // A negative pitch walks upward from the top row; start the slice at the lowest row.
            let base = if pitch < 0 {
                unsafe { scan0.offset(pitch as isize * (height as isize - 1)) }
            } else {
                scan0
            };
            convert::to_rgb(&RawFrame {
                layout: Layout::Bgrx,
                width,
                height,
                stride,
                bottom_up: pitch < 0,
                data: unsafe { std::slice::from_raw_parts(base, len) },
            })
            .map_err(StreamError::Frame)
        };

        let mut raw_2d = ptr::null_mut();
        if call!(buffer, QUERY_INTERFACE, &IID_IMF2DBUFFER => *const Guid, &mut raw_2d => *mut *mut c_void)
            >= 0
            && !raw_2d.is_null()
        {
            let buffer_2d = Com(raw_2d);
            let mut scan0: *mut u8 = ptr::null_mut();
            let mut pitch = 0i32;
            if call!(buffer_2d, BUFFER_2D_LOCK, &mut scan0 => *mut *mut u8, &mut pitch => *mut i32)
                >= 0
            {
                let result = convert_rows(scan0, pitch, None);
                call!(buffer_2d, BUFFER_2D_UNLOCK);
                return result;
            }
        }
        let mut data: *mut u8 = ptr::null_mut();
        let (mut max, mut current) = (0u32, 0u32);
        check(
            call!(buffer, BUFFER_LOCK, &mut data => *mut *mut u8, &mut max => *mut u32, &mut current => *mut u32),
            "locking a frame",
        )
        .map_err(|error| StreamError::Frame(error.to_string()))?;
        // Contiguous RGB32 buffers follow the media type's default stride.
        let pitch = if self.default_stride == 0 {
            (width * 4) as i32
        } else {
            self.default_stride
        };
        let scan0 = if pitch < 0 {
            unsafe { data.add(pitch.unsigned_abs() as usize * (height - 1)) }
        } else {
            data
        };
        let result = convert_rows(scan0, pitch, Some(current as usize));
        call!(buffer, BUFFER_UNLOCK);
        result
    }
}

impl Stream for MfStream {
    fn next_frame(&mut self, _timeout: Duration) -> std::result::Result<Option<Rgb>, StreamError> {
        // Synchronous reads return at the next frame; close interrupts them by shutting the
        // source down.
        let (mut actual, mut flags, mut timestamp) = (0u32, 0u32, 0i64);
        let mut raw = ptr::null_mut();
        let hr = call!(self.reader, READER_READ_SAMPLE,
            FIRST_VIDEO_STREAM => u32, 0 => u32, &mut actual => *mut u32,
            &mut flags => *mut u32, &mut timestamp => *mut i64, &mut raw => *mut *mut c_void);
        let sample = (!raw.is_null()).then(|| Com(raw));
        if hr < 0 {
            return Err(StreamError::Fatal(format!(
                "camera read failed (HRESULT 0x{:08x})",
                hr as u32
            )));
        }
        if flags & READERF_ERROR != 0 {
            return Err(StreamError::Fatal("camera stream failed".into()));
        }
        if flags & READERF_ENDOFSTREAM != 0 {
            return Err(StreamError::Fatal("camera stream ended".into()));
        }
        if flags & READERF_CURRENTMEDIATYPECHANGED != 0 {
            self.refresh_type()
                .map_err(|error| StreamError::Fatal(error.to_string()))?;
        }
        match sample {
            Some(sample) => self.convert(&sample).map(Some),
            None => Ok(None),
        }
    }
}

impl Drop for MfStream {
    fn drop(&mut self) {
        if let Some(source) = &self.source {
            call!(source, SOURCE_SHUTDOWN);
        }
    }
}

fn reader_attributes() -> Result<Com> {
    let mut raw = ptr::null_mut();
    check(
        unsafe { MFCreateAttributes(&mut raw, 1) },
        "MFCreateAttributes",
    )?;
    let attributes = Com::from_out(raw, "MFCreateAttributes")?;
    check(
        call!(attributes, ATTR_SET_UINT32, &MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING => *const Guid, 1 => u32),
        "enabling video processing",
    )?;
    Ok(attributes)
}

/// Shuts the media source down from another thread, ending a blocked `ReadSample`.
fn interrupt_for(source: &Com) -> Interrupt {
    let mut raw = ptr::null_mut();
    call!(source, QUERY_INTERFACE, &IID_IMFMEDIASOURCE => *const Guid, &mut raw => *mut *mut c_void);
    let source = Com(raw);
    Box::new(move || {
        if !source.0.is_null() {
            call!(source, SOURCE_SHUTDOWN);
        }
    })
}

pub type NativeCamera = ThreadedCamera;

pub fn open(device_id: &str, preferred: CameraFormat) -> Result<NativeCamera> {
    let device = list_cameras()?
        .into_iter()
        .find(|device| device.id == device_id)
        .ok_or_else(|| CameraError::DeviceNotFound(device_id.to_string()))?;
    let link = wide(device_id);
    ThreadedCamera::spawn(device, move || {
        let runtime = Runtime::start()?;
        let attributes = video_capture_attributes(2)?;
        check(
            call!(attributes, ATTR_SET_STRING,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK => *const Guid,
                link.as_ptr() => *const u16),
            "selecting the camera",
        )?;
        let mut raw = ptr::null_mut();
        check(
            unsafe { MFCreateDeviceSource(attributes.0, &mut raw) },
            "opening the camera",
        )?;
        let source = Com::from_out(raw, "MFCreateDeviceSource")?;
        let mut raw = ptr::null_mut();
        check(
            unsafe {
                MFCreateSourceReaderFromMediaSource(source.0, reader_attributes()?.0, &mut raw)
            },
            "creating the camera reader",
        )?;
        let reader = Com::from_out(raw, "MFCreateSourceReaderFromMediaSource")?;
        let interrupt = interrupt_for(&source);
        let (stream, format) = MfStream::open(Some(source), reader, runtime, preferred)?;
        Ok((stream, format, Some(interrupt)))
    })
}

/// Read a video file through the same reader path as a camera.
#[cfg(feature = "test-sources")]
pub fn open_file(path: &str, preferred: CameraFormat) -> Result<NativeCamera> {
    let device = CameraDevice {
        id: path.to_string(),
        label: path.to_string(),
        backend: CameraBackend::MediaFoundation,
        description: "video file".into(),
    };
    let url = wide(path);
    ThreadedCamera::spawn(device, move || {
        let runtime = Runtime::start()?;
        let mut raw = ptr::null_mut();
        check(
            unsafe { MFCreateSourceReaderFromURL(url.as_ptr(), reader_attributes()?.0, &mut raw) },
            "opening the video file",
        )?;
        let reader = Com::from_out(raw, "MFCreateSourceReaderFromURL")?;
        let (stream, format) = MfStream::open(None, reader, runtime, preferred)?;
        Ok((stream, format, None))
    })
}
