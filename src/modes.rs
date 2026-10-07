//! Choosing a native capture mode for backends that enumerate modes themselves.

use crate::CameraFormat;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mode {
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    /// Small tie-breaker against modes that need decoding (MJPEG, H.264).
    pub decode_cost: f64,
}

/// Decode cost for compressed native formats.
pub(crate) const COMPRESSED: f64 = 0.05;

/// Lower is better: size first, then frame rate, avoiding modes under 15fps and, among equal
/// modes, ones that need JPEG decoding. Matches the macOS backend's format choice.
fn cost(mode: &Mode, preferred: CameraFormat) -> f64 {
    let dw = f64::from(mode.width) / f64::from(preferred.width.max(1)) - 1.0;
    let dh = f64::from(mode.height) / f64::from(preferred.height.max(1)) - 1.0;
    let wanted = f64::from(preferred.frame_rate.max(1));
    let df = (mode.frame_rate - wanted).abs() / wanted;
    dw * dw
        + dh * dh
        + df * 0.25
        + if mode.frame_rate < 15.0 { 1.0 } else { 0.0 }
        + mode.decode_cost
}

/// The candidate whose mode is nearest `preferred`.
pub(crate) fn best<T>(
    candidates: impl IntoIterator<Item = (Mode, T)>,
    preferred: CameraFormat,
) -> Option<(Mode, T)> {
    candidates
        .into_iter()
        .filter(|(mode, _)| mode.width > 0 && mode.height > 0 && mode.frame_rate > 0.0)
        .min_by(|(a, _), (b, _)| cost(a, preferred).total_cmp(&cost(b, preferred)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DEFAULT_FORMAT, PixelFormat};

    #[derive(Clone, Copy)]
    enum Kind {
        Raw,
        Mjpeg,
    }

    fn mode(kind: Kind, width: u32, height: u32, frame_rate: f64) -> Mode {
        Mode {
            width,
            height,
            frame_rate,
            decode_cost: match kind {
                Kind::Raw => 0.0,
                Kind::Mjpeg => COMPRESSED,
            },
        }
    }

    #[test]
    fn prefers_720p30_then_raw_over_mjpeg_and_avoids_slow_modes() {
        let modes = [
            mode(Kind::Raw, 3840, 2160, 5.0),
            mode(Kind::Raw, 1280, 720, 10.0),
            mode(Kind::Mjpeg, 1280, 720, 30.0),
            mode(Kind::Raw, 640, 480, 30.0),
            mode(Kind::Mjpeg, 1920, 1080, 30.0),
        ];
        let pick =
            |modes: &[Mode]| best(modes.iter().map(|m| (*m, ())), DEFAULT_FORMAT).map(|m| m.0);
        assert_eq!(pick(&modes), Some(modes[2]));
        let raw = mode(Kind::Raw, 1280, 720, 30.0);
        assert_eq!(pick(&[modes[2], raw]), Some(raw));
        assert_eq!(pick(&modes[..2]), Some(modes[1]));
        assert_eq!(pick(&[]), None);
        let small = CameraFormat::new(640, 480, 30, PixelFormat::Rgb);
        assert_eq!(
            best(modes.iter().map(|m| (*m, ())), small).map(|m| m.0),
            Some(modes[3])
        );
    }
}
