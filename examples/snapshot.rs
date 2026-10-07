//! List cameras, open one, and save its next frame as a binary PPM.
//!
//! `cargo run --example snapshot -- [camera-id-or-label] [output.ppm]`
use std::{fs, time::Duration};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let wanted = args.next();
    let output = args.next().unwrap_or_else(|| "snapshot.ppm".into());

    if !camlib::request_authorization() {
        return Err(camlib::CameraError::PermissionDenied.into());
    }
    let devices = camlib::list_cameras()?;
    for device in &devices {
        println!("{}\t{}", device.id, device.display_label());
    }
    let device = match &wanted {
        Some(wanted) => devices
            .iter()
            .find(|device| &device.id == wanted || device.label.contains(wanted.as_str())),
        None => devices.first(),
    }
    .ok_or("no matching camera")?;

    let mut camera = camlib::open_camera(&device.id)?;
    // The first frames can be dark while exposure settles.
    let mut frame = camera.wait_frame(Duration::from_secs(5))?;
    for _ in 0..10 {
        frame = camera.wait_frame(Duration::from_secs(1))?;
    }
    camera.close();

    let mut ppm = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
    ppm.extend_from_slice(&frame.data);
    fs::write(&output, ppm)?;
    println!(
        "saved frame #{} ({}x{}) from {} to {output}",
        frame.sequence, frame.width, frame.height, device.label
    );
    Ok(())
}
