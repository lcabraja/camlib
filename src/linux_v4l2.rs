use crate::{
    CameraBackend, CameraDevice, CameraError, CameraFormat, PixelFormat, Result,
    convert::{self, Layout, RawFrame, Rgb},
    modes::{self, Mode},
    threaded::{Stream, StreamError, ThreadedCamera},
};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    ptr,
    time::Duration,
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeMode {
    fourcc: u32,
    width: u32,
    height: u32,
    numerator: u32,
    denominator: u32,
}

type FrameCallback = extern "C" fn(*mut c_void, *const u8, usize);

unsafe extern "C" {
    fn camlib_v4l2_list(buffer: *mut c_char, buffer_len: usize) -> usize;
    fn camlib_v4l2_modes(
        path: *const c_char,
        modes: *mut NativeMode,
        capacity: c_int,
        error: *mut c_char,
        error_len: usize,
    ) -> c_int;
    fn camlib_v4l2_open(
        path: *const c_char,
        mode: *mut NativeMode,
        bytes_per_line: *mut u32,
        error: *mut c_char,
        error_len: usize,
    ) -> *mut c_void;
    fn camlib_v4l2_read(
        camera: *mut c_void,
        timeout_ms: c_int,
        frame_cb: FrameCallback,
        ctx: *mut c_void,
        error: *mut c_char,
        error_len: usize,
    ) -> c_int;
    fn camlib_v4l2_close(camera: *mut c_void);
}

#[cfg(test)]
const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}

fn layout(code: u32) -> Option<Layout> {
    Some(match &code.to_le_bytes() {
        b"YUYV" => Layout::Yuyv,
        b"UYVY" => Layout::Uyvy,
        b"YVYU" => Layout::Yvyu,
        b"NV12" => Layout::Nv12,
        b"NV21" => Layout::Nv21,
        b"YU12" => Layout::I420,
        b"YV12" => Layout::Yv12,
        b"RGB3" => Layout::Rgb24,
        b"BGR3" => Layout::Bgr24,
        // XBGR32, ABGR32 and the legacy BGR32 all store B, G, R, X in memory.
        b"XR24" | b"AR24" | b"BGR4" => Layout::Bgrx,
        b"GREY" => Layout::Gray,
        b"MJPG" | b"JPEG" => Layout::Mjpeg,
        _ => return None,
    })
}

fn error_text(buffer: &[c_char]) -> String {
    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

pub fn list_cameras() -> Result<Vec<CameraDevice>> {
    let mut capacity = unsafe { camlib_v4l2_list(ptr::null_mut(), 0) };
    let buffer = loop {
        let mut buffer = vec![0u8; capacity];
        let required = unsafe { camlib_v4l2_list(buffer.as_mut_ptr().cast(), buffer.len()) };
        if required <= capacity {
            break buffer;
        }
        capacity = required;
    };
    let text = CStr::from_bytes_until_nul(&buffer)
        .map_err(|error| CameraError::Native(error.to_string()))?
        .to_string_lossy();
    Ok(text
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            Some(CameraDevice {
                id: parts.next()?.to_string(),
                label: parts.next()?.trim().to_string(),
                backend: CameraBackend::Video4Linux,
                description: parts.next().unwrap_or_default().trim().to_string(),
            })
        })
        .collect())
}

fn native_modes(path: &CStr) -> Result<Vec<NativeMode>> {
    let mut error = [0 as c_char; 256];
    let mut modes = vec![NativeMode::default(); 256];
    loop {
        let count = unsafe {
            camlib_v4l2_modes(
                path.as_ptr(),
                modes.as_mut_ptr(),
                modes.len() as c_int,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if count < 0 {
            return Err(CameraError::Native(error_text(&error)));
        }
        let count = count as usize;
        if count <= modes.len() {
            modes.truncate(count);
            return Ok(modes);
        }
        modes.resize(count, NativeMode::default());
    }
}

struct V4l2Stream {
    handle: *mut c_void,
    layout: Layout,
    width: usize,
    height: usize,
    stride: usize,
}

// The handle is only used by the capture thread that owns this stream.
unsafe impl Send for V4l2Stream {}

struct Delivery<'a> {
    stream: &'a V4l2Stream,
    result: Option<std::result::Result<Rgb, String>>,
}

extern "C" fn deliver(ctx: *mut c_void, data: *const u8, length: usize) {
    let delivery = unsafe { &mut *ctx.cast::<Delivery<'_>>() };
    // SAFETY: the buffer stays mapped and dequeued for the duration of the callback.
    let data = unsafe { std::slice::from_raw_parts(data, length) };
    let stream = delivery.stream;
    delivery.result = Some(convert::to_rgb(&RawFrame {
        layout: stream.layout,
        width: stream.width,
        height: stream.height,
        stride: stream.stride,
        bottom_up: false,
        data,
    }));
}

impl Stream for V4l2Stream {
    fn next_frame(&mut self, timeout: Duration) -> std::result::Result<Option<Rgb>, StreamError> {
        let mut error = [0 as c_char; 256];
        let mut delivery = Delivery {
            stream: self,
            result: None,
        };
        let status = unsafe {
            camlib_v4l2_read(
                self.handle,
                timeout.as_millis().min(i32::MAX as u128) as c_int,
                deliver,
                (&raw mut delivery).cast(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        match status {
            1 => match delivery.result {
                Some(Ok(frame)) => Ok(Some(frame)),
                Some(Err(message)) => {
                    Err(StreamError::Frame(format!("skipped a frame: {message}")))
                }
                None => Ok(None),
            },
            0 => Ok(None),
            -2 => Err(StreamError::Fatal(format!(
                "camera disconnected ({})",
                error_text(&error)
            ))),
            _ => Err(StreamError::Fatal(error_text(&error))),
        }
    }
}

impl Drop for V4l2Stream {
    fn drop(&mut self) {
        unsafe { camlib_v4l2_close(self.handle) };
    }
}

pub type NativeCamera = ThreadedCamera;

pub fn open(device_id: &str, preferred: CameraFormat) -> Result<NativeCamera> {
    open_filtered(device_id, preferred, None)
}

/// Open using only the native pixel format `only` (a V4L2 fourcc), for testing converters.
#[cfg(feature = "test-sources")]
pub fn open_fourcc(
    device_id: &str,
    preferred: CameraFormat,
    only: [u8; 4],
) -> Result<NativeCamera> {
    open_filtered(device_id, preferred, Some(u32::from_le_bytes(only)))
}

fn open_filtered(
    device_id: &str,
    preferred: CameraFormat,
    only: Option<u32>,
) -> Result<NativeCamera> {
    let device = list_cameras()?
        .into_iter()
        .find(|device| device.id == device_id)
        .ok_or_else(|| CameraError::DeviceNotFound(device_id.to_string()))?;
    let path =
        CString::new(device_id).map_err(|error| CameraError::InvalidInput(error.to_string()))?;
    ThreadedCamera::spawn(device, move || {
        let candidates = native_modes(&path)?.into_iter().filter_map(|native| {
            if only.is_some_and(|only| only != native.fourcc) {
                return None;
            }
            let mode = Mode {
                width: native.width,
                height: native.height,
                frame_rate: f64::from(native.denominator) / f64::from(native.numerator),
                decode_cost: layout(native.fourcc)?.decode_cost(),
            };
            Some((mode, native))
        });
        let (_, mut native) = modes::best(candidates, preferred).ok_or_else(|| {
            CameraError::Native("camera offers no pixel format camlib can decode".into())
        })?;
        let mut stride = 0u32;
        let mut error = [0 as c_char; 256];
        let handle = unsafe {
            camlib_v4l2_open(
                path.as_ptr(),
                &mut native,
                &mut stride,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if handle.is_null() {
            return Err(CameraError::Native(error_text(&error)));
        }
        // The driver may substitute a format; decode what it actually delivers.
        let Some(layout) = layout(native.fourcc) else {
            unsafe { camlib_v4l2_close(handle) };
            return Err(CameraError::Native(format!(
                "camera switched to unsupported pixel format {:?}",
                String::from_utf8_lossy(&native.fourcc.to_le_bytes())
            )));
        };
        let frame_rate = if native.numerator == 0 {
            0
        } else {
            (f64::from(native.denominator) / f64::from(native.numerator)).round() as u32
        };
        let format = CameraFormat::new(native.width, native.height, frame_rate, PixelFormat::Rgb);
        let stream = V4l2Stream {
            handle,
            layout,
            width: native.width as usize,
            height: native.height as usize,
            stride: stride as usize,
        };
        Ok((stream, format, None))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourccs_map_to_layouts() {
        assert_eq!(layout(fourcc(b"YUYV")), Some(Layout::Yuyv));
        assert_eq!(layout(fourcc(b"MJPG")), Some(Layout::Mjpeg));
        assert_eq!(layout(fourcc(b"H264")), None);
    }
}
