# camlib

Small zero-dependency Rust camera library distilled from OBS Studio's camera source flow.

## Goals

- No Rust crate dependencies.
- Native OS camera backends, not a generic webcam wrapper.
- A small API for listing cameras, opening by stable id, reading RGB frames, and closing.
- A tiny example app that delegates UI and camera behavior to the library.

OBS keeps capture native per OS:

- Windows: DirectShow device enumeration/opening in `plugins/win-dshow/win-dshow.cpp`.
- macOS: AVFoundation discovery/session setup in `plugins/mac-avcapture`.
- Linux: V4L2 device discovery/open/stream lifecycle in `plugins/linux-v4l2/v4l2-input.c`.

This crate keeps that logic shape:

1. Enumerate native cameras into stable `id` plus human `label`.
2. Select a device by id.
3. Open the OS-native stream.
4. Poll frames.
5. Close the stream and release the backend handle.

There are no Rust crate dependencies. Native OS integration is compiled directly by `build.rs`.
The current implemented backend is macOS AVFoundation; Linux V4L2 and Windows capture are explicit
future native backends, not hidden behind a generic camera crate.

## API Sketch

```rust
let devices = camlib::list_cameras()?;
let mut camera = camlib::open_camera(&devices[0].id)?;
let frame = camera.frame_rgb()?;
camera.close();
# Ok::<(), camlib::CameraError>(())
```

## Run the picker

```sh
cargo run --example picker
```

The example is a tiny launcher into the library-owned native picker. On macOS it opens a Cocoa
window with a camera picker, `Open`/`Close` buttons, and an AVFoundation BGRA preview path inspired
by OBS's `mac-avcapture` plugin.
