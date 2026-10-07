# camlib

Zero-dependency Rust camera capture, distilled from OBS Studio's native camera sources. Each OS
uses its own backend, compiled directly by `build.rs`; there are no Rust crate dependencies.

| OS | Backend | Notes |
|---|---|---|
| macOS | AVFoundation, after OBS `plugins/mac-avcapture` | BGRA from the system |
| Linux | V4L2, after OBS `plugins/linux-v4l2` | YUYV, UYVY, YVYU, NV12, NV21, I420, YV12, RGB, BGR, BGRX, grey and MJPEG |
| Windows | Media Foundation source reader | decodes MJPEG/H.264 and converts to RGB itself |

MJPEG is decoded by camlib's own baseline JPEG decoder (bit-exact with libjpeg, about 8 ms per
720p frame in release builds). Every backend picks the native mode nearest the request.

```rust
use std::time::Duration;

if !camlib::request_authorization() {
    return Err(camlib::CameraError::PermissionDenied);
}
let devices = camlib::list_cameras()?;
// Opens the native mode nearest 1280x720@30; `open_camera_with` prefers another.
let mut camera = camlib::open_camera(&devices[0].id)?;
// Returns a frame newer than the last one, or `Timeout` / `Disconnected`.
let frame = camera.wait_frame(Duration::from_secs(1))?; // packed RGB
camera.close();
# Ok::<(), camlib::CameraError>(())
```

`request_authorization` may show the OS prompt and blocks, so call it off the UI thread.
`OpenedCamera` is `Send`; `close` (or drop) waits for in-flight native callbacks.

## macOS notes

- Apps need `NSCameraUsageDescription`, plus `NSCameraUseContinuityCameraDeviceType` for iPhone
  cameras. Permission belongs to the responsible app: a CLI inherits its terminal's permission.
- If another app holds a device's configuration lock, the camera is shared in its current mode
  and `status()` says so.

## Examples

```sh
cargo run --example snapshot -- "FaceTime" frame.ppm   # save one frame
cargo run --example picker                             # native picker window (macOS)
```

## Testing without a camera

The `test-sources` feature adds hidden entry points used by CI: on Linux, the `v4l2_formats`
example captures every pixel format from the kernel's `vivid` virtual camera and compares it
with RGB24; on Windows, `file_source` reads a video file through the camera reader path.

## License

GPL-2.0-or-later, the same terms as the OBS Studio sources this library follows. Applications that
link camlib are distributed under GPL-compatible terms.
