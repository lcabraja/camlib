use crate::{
    Authorization, CameraBackend, CameraDevice, CameraError, CameraFormat, PixelFormat, Result,
    RgbFrame,
    convert::{self, Layout, RawFrame},
    frames::FrameSlot,
};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    ptr,
    time::Duration,
};

type FrameCallback = extern "C" fn(*mut c_void, *const u8, i32, i32, usize);
type StatusCallback = extern "C" fn(*mut c_void, c_int, *const c_char);

const STATUS_ERROR: c_int = 1;
const STATUS_DISCONNECTED: c_int = 2;

#[repr(C)]
struct NativeFormat {
    width: c_int,
    height: c_int,
    frame_rate: f64,
}

unsafe extern "C" {
    fn camlib_avf_authorization() -> c_int;
    fn camlib_avf_request_access() -> c_int;
    fn camlib_avf_list(buffer: *mut c_char, buffer_len: usize) -> usize;
    fn camlib_avf_open(
        unique_id: *const c_char,
        format: *mut NativeFormat,
        frame_cb: FrameCallback,
        status_cb: StatusCallback,
        ctx: *mut c_void,
        error: *mut c_char,
        error_len: usize,
    ) -> *mut c_void;
    fn camlib_avf_close(handle: *mut c_void);
    fn camlib_avf_run_picker();
}

pub fn authorization() -> Authorization {
    match unsafe { camlib_avf_authorization() } {
        0 => Authorization::Authorized,
        1 => Authorization::NotDetermined,
        3 => Authorization::Restricted,
        _ => Authorization::Denied,
    }
}

pub fn request_authorization() -> bool {
    unsafe { camlib_avf_request_access() != 0 }
}

pub fn list_cameras() -> Result<Vec<CameraDevice>> {
    // The device list can grow between the size query and the copy; retry until it fits.
    let mut capacity = unsafe { camlib_avf_list(ptr::null_mut(), 0) };
    let buffer = loop {
        let mut buffer = vec![0u8; capacity];
        let required = unsafe { camlib_avf_list(buffer.as_mut_ptr().cast(), buffer.len()) };
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
                label: parts.next()?.to_string(),
                backend: CameraBackend::AvFoundation,
                description: parts.next().unwrap_or_default().to_string(),
            })
        })
        .collect())
}

pub struct NativeCamera {
    device: CameraDevice,
    format: CameraFormat,
    handle: *mut c_void,
    // Owned by this struct; native callbacks borrow it until `camlib_avf_close` returns.
    state: *mut FrameSlot,
    returned: u64,
}

// AVFoundation sessions may be started and stopped from any thread, and the callback state is
// synchronized, so an open camera can move to the thread that consumes its frames.
unsafe impl Send for NativeCamera {}

pub fn open(device_id: &str, preferred: CameraFormat) -> Result<NativeCamera> {
    NativeCamera::open(device_id, preferred)
}

impl NativeCamera {
    pub fn open(device_id: &str, preferred: CameraFormat) -> Result<Self> {
        if authorization() != Authorization::Authorized {
            return Err(CameraError::PermissionDenied);
        }
        let device = list_cameras()?
            .into_iter()
            .find(|device| device.id == device_id)
            .ok_or_else(|| CameraError::DeviceNotFound(device_id.to_string()))?;
        let id = CString::new(device_id)
            .map_err(|error| CameraError::InvalidInput(error.to_string()))?;
        let state = Box::into_raw(Box::new(FrameSlot::default()));
        let mut error = [0 as c_char; 512];
        let clamp = |value: u32| c_int::try_from(value).unwrap_or(c_int::MAX);
        let mut format = NativeFormat {
            width: clamp(preferred.width),
            height: clamp(preferred.height),
            frame_rate: f64::from(preferred.frame_rate),
        };
        let handle = unsafe {
            camlib_avf_open(
                id.as_ptr(),
                &mut format,
                frame_callback,
                status_callback,
                state.cast::<c_void>(),
                error.as_mut_ptr(),
                error.len(),
            )
        };

        if handle.is_null() {
            unsafe {
                drop(Box::from_raw(state));
            }
            let message = unsafe { CStr::from_ptr(error.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            return Err(CameraError::Native(if message.is_empty() {
                format!("could not open camera '{}'", device.label)
            } else {
                message
            }));
        }

        Ok(Self {
            device,
            format: CameraFormat::new(
                u32::try_from(format.width).unwrap_or(0),
                u32::try_from(format.height).unwrap_or(0),
                format.frame_rate.round() as u32,
                PixelFormat::Rgb,
            ),
            handle,
            state,
            returned: 0,
        })
    }

    pub fn device(&self) -> &CameraDevice {
        &self.device
    }

    pub fn format(&self) -> CameraFormat {
        self.format
    }

    pub fn status(&self) -> Option<String> {
        self.state()?.status()
    }

    pub fn frame_rgb(&mut self) -> Result<RgbFrame> {
        let frame = self.state().ok_or(CameraError::NoFrame)?.latest()?;
        self.accept(&frame);
        Ok(frame)
    }

    pub fn wait_frame(&mut self, timeout: Duration) -> Result<RgbFrame> {
        let frame = self
            .state()
            .ok_or(CameraError::NoFrame)?
            .wait_newer(self.returned, timeout)?;
        self.accept(&frame);
        Ok(frame)
    }

    pub fn close(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                // Returns only after in-flight callbacks drain, so the state can be freed.
                camlib_avf_close(self.handle);
                drop(Box::from_raw(self.state));
            }
            self.handle = ptr::null_mut();
            self.state = ptr::null_mut();
        }
    }

    fn state(&self) -> Option<&FrameSlot> {
        unsafe { self.state.as_ref() }
    }

    fn accept(&mut self, frame: &RgbFrame) {
        self.returned = frame.sequence;
        self.format.width = frame.width;
        self.format.height = frame.height;
    }
}

impl Drop for NativeCamera {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn run_camera_picker() {
    unsafe {
        camlib_avf_run_picker();
    }
}

extern "C" fn frame_callback(
    ctx: *mut c_void,
    bgra: *const u8,
    width: i32,
    height: i32,
    bytes_per_row: usize,
) {
    if ctx.is_null() || bgra.is_null() || width <= 0 || height <= 0 {
        return;
    }

    // SAFETY: the backend keeps `height` rows of `bytes_per_row` bytes locked for the call.
    let data = unsafe { std::slice::from_raw_parts(bgra, bytes_per_row * height as usize) };
    let slot = unsafe { &*(ctx.cast::<FrameSlot>()) };
    match convert::to_rgb(&RawFrame {
        layout: Layout::Bgrx,
        width: width as usize,
        height: height as usize,
        stride: bytes_per_row,
        bottom_up: false,
        data,
    }) {
        Ok((width, height, rgb)) => slot.publish(width, height, rgb),
        Err(message) => slot.report(message, false),
    }
}

extern "C" fn status_callback(ctx: *mut c_void, kind: c_int, message: *const c_char) {
    if ctx.is_null() || message.is_null() {
        return;
    }

    let slot = unsafe { &*(ctx.cast::<FrameSlot>()) };
    let message = unsafe { CStr::from_ptr(message) }
        .to_string_lossy()
        .into_owned();
    slot.report(message, matches!(kind, STATUS_ERROR | STATUS_DISCONNECTED));
}
