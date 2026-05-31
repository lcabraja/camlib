use crate::{
    CameraBackend, CameraDevice, CameraError, CameraFormat, PixelFormat, Result, RgbFrame,
};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    ptr,
    sync::{Arc, Mutex},
};

type FrameCallback = extern "C" fn(*mut c_void, *const u8, i32, i32, usize);
type StatusCallback = extern "C" fn(*mut c_void, *const c_char);

unsafe extern "C" {
    fn camlib_avf_list(buffer: *mut c_char, buffer_len: usize) -> usize;
    fn camlib_avf_open(
        unique_id: *const c_char,
        frame_cb: FrameCallback,
        status_cb: StatusCallback,
        ctx: *mut c_void,
    ) -> *mut c_void;
    fn camlib_avf_close(handle: *mut c_void);
    fn camlib_avf_run_picker();
}

pub fn list_cameras() -> Result<Vec<CameraDevice>> {
    let required = unsafe { camlib_avf_list(ptr::null_mut(), 0) };
    if required == 0 {
        return Ok(Vec::new());
    }

    let mut buffer = vec![0u8; required];
    unsafe {
        camlib_avf_list(buffer.as_mut_ptr().cast(), buffer.len());
    }

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

struct CallbackState {
    latest: Arc<Mutex<Option<RgbFrame>>>,
    status: Arc<Mutex<Option<String>>>,
}

pub struct NativeCamera {
    device: CameraDevice,
    format: CameraFormat,
    handle: *mut c_void,
    state: *mut CallbackState,
    latest: Arc<Mutex<Option<RgbFrame>>>,
}

impl NativeCamera {
    pub fn open(device_id: &str) -> Result<Self> {
        let device = list_cameras()?
            .into_iter()
            .find(|device| device.id == device_id)
            .ok_or_else(|| CameraError::DeviceNotFound(device_id.to_string()))?;
        let id = CString::new(device_id)
            .map_err(|error| CameraError::InvalidInput(error.to_string()))?;
        let latest = Arc::new(Mutex::new(None));
        let status = Arc::new(Mutex::new(None));
        let state = Box::into_raw(Box::new(CallbackState {
            latest: Arc::clone(&latest),
            status,
        }));
        let handle = unsafe {
            camlib_avf_open(
                id.as_ptr(),
                frame_callback,
                status_callback,
                state.cast::<c_void>(),
            )
        };

        if handle.is_null() {
            unsafe {
                drop(Box::from_raw(state));
            }
            return Err(CameraError::DeviceNotFound(device_id.to_string()));
        }

        Ok(Self {
            device,
            format: CameraFormat::new(0, 0, 0, PixelFormat::Bgra),
            handle,
            state,
            latest,
        })
    }

    pub fn device(&self) -> &CameraDevice {
        &self.device
    }

    pub fn format(&self) -> CameraFormat {
        self.format
    }

    pub fn frame_rgb(&mut self) -> Result<RgbFrame> {
        let frame = self
            .latest
            .lock()
            .map_err(|error| CameraError::Native(error.to_string()))?
            .clone()
            .ok_or(CameraError::NoFrame)?;
        self.format.width = frame.width;
        self.format.height = frame.height;
        Ok(frame)
    }

    pub fn close(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                camlib_avf_close(self.handle);
                drop(Box::from_raw(self.state));
            }
            self.handle = ptr::null_mut();
            self.state = ptr::null_mut();
        }
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

    let width = width as usize;
    let height = height as usize;
    let mut rgb = vec![0u8; width * height * 3];

    unsafe {
        for y in 0..height {
            let src_row = bgra.add(y * bytes_per_row);
            let dst_row = &mut rgb[y * width * 3..(y + 1) * width * 3];
            for x in 0..width {
                let src = src_row.add(x * 4);
                let dst = &mut dst_row[x * 3..x * 3 + 3];
                dst[0] = *src.add(2);
                dst[1] = *src.add(1);
                dst[2] = *src;
            }
        }

        let state = &*(ctx.cast::<CallbackState>());
        if let Ok(mut latest) = state.latest.lock() {
            *latest = Some(RgbFrame {
                width: width as u32,
                height: height as u32,
                data: rgb,
            });
        }
    }
}

extern "C" fn status_callback(ctx: *mut c_void, message: *const c_char) {
    if ctx.is_null() || message.is_null() {
        return;
    }

    let state = unsafe { &*(ctx.cast::<CallbackState>()) };
    let message = unsafe { CStr::from_ptr(message) }
        .to_string_lossy()
        .to_string();
    if let Ok(mut status) = state.status.lock() {
        *status = Some(message);
    }
}
