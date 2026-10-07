//! Platforms without a native backend: every call reports `UnsupportedPlatform`.

use crate::{CameraDevice, CameraError, CameraFormat, Result, RgbFrame};
use std::time::Duration;

pub enum NativeCamera {}

impl NativeCamera {
    pub fn device(&self) -> &CameraDevice {
        match *self {}
    }

    pub fn format(&self) -> CameraFormat {
        match *self {}
    }

    pub fn status(&self) -> Option<String> {
        match *self {}
    }

    pub fn frame_rgb(&mut self) -> Result<RgbFrame> {
        match *self {}
    }

    pub fn wait_frame(&mut self, _timeout: Duration) -> Result<RgbFrame> {
        match *self {}
    }

    pub fn close(&mut self) {
        match *self {}
    }
}

pub fn list_cameras() -> Result<Vec<CameraDevice>> {
    Err(CameraError::UnsupportedPlatform(std::env::consts::OS))
}

pub fn open(_device_id: &str, _preferred: CameraFormat) -> Result<NativeCamera> {
    Err(CameraError::UnsupportedPlatform(std::env::consts::OS))
}
