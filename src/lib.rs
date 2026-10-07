//! Zero-dependency, OBS-inspired native camera access.
//!
//! OBS does not use one generic camera abstraction. It keeps capture native:
//! DirectShow/Media Foundation-style device IDs on Windows, AVFoundation sessions on macOS,
//! and V4L2 file descriptors/streams on Linux. This crate follows that shape: a small Rust API
//! with native implementations behind `cfg`, and no third-party Rust crates.

use std::{error::Error, fmt, time::Duration};

#[cfg(target_os = "macos")]
mod macos_avfoundation;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraDevice {
    pub id: String,
    pub label: String,
    pub backend: CameraBackend,
    pub description: String,
}

impl CameraDevice {
    #[must_use]
    pub fn display_label(&self) -> String {
        if self.description.is_empty() {
            self.label.clone()
        } else {
            format!("{} ({})", self.label, self.description)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraBackend {
    AvFoundation,
    MediaFoundation,
    Video4Linux,
}

impl fmt::Display for CameraBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AvFoundation => f.write_str("AVFoundation"),
            Self::MediaFoundation => f.write_str("Media Foundation"),
            Self::Video4Linux => f.write_str("V4L2"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CameraFormat {
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub pixel_format: PixelFormat,
}

impl CameraFormat {
    #[must_use]
    pub const fn new(width: u32, height: u32, frame_rate: u32, pixel_format: PixelFormat) -> Self {
        Self {
            width,
            height,
            frame_rate,
            pixel_format,
        }
    }
}

impl fmt::Display for CameraFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.frame_rate == 0 {
            write!(f, "{}x{} {}", self.width, self.height, self.pixel_format)
        } else {
            write!(
                f,
                "{}x{}@{}fps {}",
                self.width, self.height, self.frame_rate, self.pixel_format
            )
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    Bgra,
    Rgb,
    Unknown,
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bgra => f.write_str("BGRA"),
            Self::Rgb => f.write_str("RGB"),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbFrame {
    pub width: u32,
    pub height: u32,
    /// Increases by one for every native frame delivered by the open camera.
    pub sequence: u64,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub enum CameraError {
    UnsupportedPlatform(&'static str),
    PermissionDenied,
    DeviceNotFound(String),
    InvalidInput(String),
    Native(String),
    Disconnected(String),
    NoFrame,
    Timeout,
}

impl fmt::Display for CameraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform(platform) => {
                write!(f, "camera backend is not implemented for {platform}")
            }
            Self::PermissionDenied => f.write_str("camera access is not authorized"),
            Self::DeviceNotFound(id) => write!(f, "camera '{id}' was not found"),
            Self::InvalidInput(message) | Self::Native(message) => f.write_str(message),
            Self::Disconnected(message) => write!(f, "camera disconnected: {message}"),
            Self::NoFrame => f.write_str("no frame has been received yet"),
            Self::Timeout => f.write_str("timed out waiting for a camera frame"),
        }
    }
}

impl Error for CameraError {}

pub type Result<T> = std::result::Result<T, CameraError>;

/// The operating system's camera permission for this process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Authorization {
    Authorized,
    NotDetermined,
    Denied,
    Restricted,
}

#[must_use]
pub fn native_backend() -> Option<CameraBackend> {
    if cfg!(target_os = "macos") {
        Some(CameraBackend::AvFoundation)
    } else if cfg!(target_os = "linux") {
        Some(CameraBackend::Video4Linux)
    } else if cfg!(target_os = "windows") {
        Some(CameraBackend::MediaFoundation)
    } else {
        None
    }
}

/// Current camera permission. Platforms without a permission model report `Authorized`.
#[must_use]
pub fn authorization() -> Authorization {
    #[cfg(target_os = "macos")]
    {
        macos_avfoundation::authorization()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Authorization::Authorized
    }
}

/// Ask the OS for camera access, showing its prompt if the user has not decided yet.
///
/// Blocks until the user answers, so call it off the UI thread. Returns whether access is granted.
pub fn request_authorization() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos_avfoundation::request_authorization()
    }

    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

pub fn list_cameras() -> Result<Vec<CameraDevice>> {
    #[cfg(target_os = "macos")]
    {
        macos_avfoundation::list_cameras()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(CameraError::UnsupportedPlatform(std::env::consts::OS))
    }
}

/// The mode `open_camera` asks for: 720p at 30fps, a good balance for most webcams.
pub const DEFAULT_FORMAT: CameraFormat = CameraFormat::new(1280, 720, 30, PixelFormat::Rgb);

/// Open a camera by its stable id and start streaming near [`DEFAULT_FORMAT`].
pub fn open_camera(device_id: impl AsRef<str>) -> Result<OpenedCamera> {
    open_camera_with(device_id, DEFAULT_FORMAT)
}

/// Open a camera, selecting the advertised native mode nearest `preferred`.
///
/// Size is matched before frame rate, and modes under 15fps are avoided. Frames are always
/// delivered as RGB; `preferred.pixel_format` is ignored.
pub fn open_camera_with(
    device_id: impl AsRef<str>,
    preferred: CameraFormat,
) -> Result<OpenedCamera> {
    #[cfg(target_os = "macos")]
    {
        macos_avfoundation::NativeCamera::open(device_id.as_ref(), preferred)
            .map(|native| OpenedCamera { native })
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (device_id, preferred);
        Err(CameraError::UnsupportedPlatform(std::env::consts::OS))
    }
}

#[cfg(not(target_os = "macos"))]
enum NoBackend {}

pub struct OpenedCamera {
    #[cfg(target_os = "macos")]
    native: macos_avfoundation::NativeCamera,
    #[cfg(not(target_os = "macos"))]
    native: NoBackend,
}

impl OpenedCamera {
    #[must_use]
    pub fn device(&self) -> &CameraDevice {
        #[cfg(target_os = "macos")]
        {
            self.native.device()
        }
        #[cfg(not(target_os = "macos"))]
        {
            match self.native {}
        }
    }

    /// The selected native mode, updated to the delivered size once frames arrive.
    #[must_use]
    pub fn format(&self) -> CameraFormat {
        #[cfg(target_os = "macos")]
        {
            self.native.format()
        }
        #[cfg(not(target_os = "macos"))]
        {
            match self.native {}
        }
    }

    /// The newest frame, which may be one already returned.
    pub fn frame_rgb(&mut self) -> Result<RgbFrame> {
        #[cfg(target_os = "macos")]
        {
            self.native.frame_rgb()
        }
        #[cfg(not(target_os = "macos"))]
        {
            match self.native {}
        }
    }

    /// Wait for a frame newer than any this camera has returned.
    ///
    /// Fails with `Timeout` if none arrives in time and with `Disconnected` once the device is
    /// gone or the native session fails.
    pub fn wait_frame(&mut self, timeout: Duration) -> Result<RgbFrame> {
        #[cfg(target_os = "macos")]
        {
            self.native.wait_frame(timeout)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = timeout;
            match self.native {}
        }
    }

    /// The latest native status message, if the backend reported one.
    #[must_use]
    pub fn status(&self) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            self.native.status()
        }
        #[cfg(not(target_os = "macos"))]
        {
            match self.native {}
        }
    }

    /// Stop streaming and release the device. Safe to call more than once.
    pub fn close(&mut self) {
        #[cfg(target_os = "macos")]
        {
            self.native.close();
        }
        #[cfg(not(target_os = "macos"))]
        {
            match self.native {}
        }
    }
}

impl Drop for OpenedCamera {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn run_camera_picker() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos_avfoundation::run_camera_picker();
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(CameraError::UnsupportedPlatform(std::env::consts::OS))
    }
}

#[must_use]
pub fn obs_style_lifecycle_summary() -> &'static str {
    "enumerate native devices -> select stable native id -> create native session/stream -> request native output format -> start stream -> receive frames -> stop and release native handle"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_display_includes_dimensions() {
        assert_eq!(
            CameraFormat::new(1280, 720, 30, PixelFormat::Bgra).to_string(),
            "1280x720@30fps BGRA"
        );
    }
}
