# camlib

Zero-dependency Rust camera capture, distilled from OBS Studio's native camera sources. Each OS
uses its own backend, compiled directly by `build.rs`; there are no Rust crate dependencies.

| OS | Backend | Status |
|---|---|---|
| macOS | AVFoundation (`plugins/mac-avcapture`) | implemented |
| Linux | V4L2 (`plugins/linux-v4l2`) | planned |
| Windows | DirectShow / Media Foundation (`plugins/win-dshow`) | planned |

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
cargo run --example picker                             # native picker window
```
