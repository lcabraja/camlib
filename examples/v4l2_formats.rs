//! Capture one frame per pixel format from a V4L2 camera and compare each with the camera's
//! RGB24 output. Run against the kernel's `vivid` test driver to check every converter on a real
//! device: `cargo run --features test-sources --example v4l2_formats -- vivid`
#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;
    let wanted = std::env::args().nth(1).unwrap_or_else(|| "vivid".into());
    let device = camlib::list_cameras()?
        .into_iter()
        .find(|d| d.id == wanted || d.label.contains(&wanted))
        .ok_or("no matching camera")?;
    println!("{} ({})", device.label, device.id);
    let request = camlib::CameraFormat::new(640, 480, 30, camlib::PixelFormat::Rgb);
    let grab = |fourcc: &[u8; 4]| -> Result<camlib::RgbFrame, camlib::CameraError> {
        let mut camera = camlib::open_camera_with_fourcc(&device.id, request, *fourcc)?;
        // Skip a few frames so the pattern generator is warmed up.
        let mut frame = camera.wait_frame(Duration::from_secs(5))?;
        for _ in 0..3 {
            frame = camera.wait_frame(Duration::from_secs(5))?;
        }
        Ok(frame)
    };
    let reference = grab(b"RGB3")?;
    std::fs::write(
        "v4l2-reference.ppm",
        [
            format!("P6\n{} {}\n255\n", reference.width, reference.height).as_bytes(),
            &reference.data,
        ]
        .concat(),
    )?;
    let mut failures = 0;
    for fourcc in [
        b"YUYV", b"UYVY", b"YVYU", b"NV12", b"NV21", b"YU12", b"YV12", b"BGR3", b"XR24", b"AR24",
        b"GREY", b"MJPG",
    ] {
        let name = String::from_utf8_lossy(fourcc);
        match grab(fourcc) {
            Ok(frame) if (frame.width, frame.height) != (reference.width, reference.height) => {
                println!(
                    "{name}: size {}x{} differs from reference",
                    frame.width, frame.height
                );
                failures += 1;
            }
            Ok(frame) => {
                // Moving text differs between frames; compare the whole-frame mean error.
                let gray = fourcc == b"GREY";
                let total: u64 = frame
                    .data
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .zip(reference.data.as_chunks::<3>().0.iter())
                    .map(|(a, b)| {
                        if gray {
                            let luma = (u32::from(b[0]) * 299
                                + u32::from(b[1]) * 587
                                + u32::from(b[2]) * 114)
                                / 1000;
                            u64::from(a[0].abs_diff(luma as u8))
                        } else {
                            a.iter()
                                .zip(b)
                                .map(|(x, y)| u64::from(x.abs_diff(*y)))
                                .sum::<u64>()
                                / 3
                        }
                    })
                    .sum();
                let mean = total as f64 / (frame.width * frame.height) as f64;
                let ok = mean < 8.0;
                println!(
                    "{name}: mean error {mean:.2} {}",
                    if ok { "ok" } else { "FAIL" }
                );
                failures += usize::from(!ok);
            }
            Err(camlib::CameraError::Native(message)) if message.contains("no pixel format") => {
                println!("{name}: not offered by this camera");
            }
            Err(error) => {
                println!("{name}: {error}");
                failures += 1;
            }
        }
    }
    if failures > 0 {
        return Err(format!("{failures} pixel format(s) failed").into());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("v4l2_formats exercises the Linux backend only");
}
