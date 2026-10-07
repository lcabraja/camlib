//! Read a video file through the Windows camera pipeline and save its third frame as a PPM,
//! optionally checking it against a raw RGB reference. Tests the Media Foundation backend
//! without a camera:
//!
//! `cargo run --features test-sources --example file_source -- tests/fixtures/clip.mp4 frame.ppm tests/fixtures/clip-frame3.rgb`
#[cfg(target_os = "windows")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: file_source <video> <output.ppm>")?;
    let output = args.next().unwrap_or_else(|| "frame.ppm".into());
    let path = std::fs::canonicalize(&input)?;
    // Media Foundation wants a plain path, not the \\?\ form canonicalize returns.
    let path = path
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string();
    let mut source = camlib::open_video_file(&path, camlib::DEFAULT_FORMAT)?;
    println!("opened {path} as {}", source.format());
    let mut frame = source.wait_frame(Duration::from_secs(10))?;
    for _ in 0..2 {
        frame = source.wait_frame(Duration::from_secs(10))?;
    }
    let mut ppm = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
    ppm.extend_from_slice(&frame.data);
    std::fs::write(&output, ppm)?;
    println!(
        "saved frame #{} ({}x{}) to {output}",
        frame.sequence, frame.width, frame.height
    );
    if let Some(reference) = args.next() {
        let reference = std::fs::read(reference)?;
        if reference.len() != frame.data.len() {
            return Err(format!(
                "reference is {} bytes, frame is {}",
                reference.len(),
                frame.data.len()
            )
            .into());
        }
        let total: u64 = frame
            .data
            .iter()
            .zip(&reference)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        let mean = total as f64 / reference.len() as f64;
        // Decoders round YUV differently; a flipped or misread frame is far above this.
        println!("mean error against reference: {mean:.2}");
        if mean > 8.0 {
            return Err("frame does not match the reference".into());
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("file_source exercises the Windows backend only");
}
