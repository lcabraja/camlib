//! Native camera pixel layouts and their conversion to packed RGB.

use crate::jpeg;

/// Pixel layouts camlib can decode. Planar and packed YUV use BT.601 limited range, the norm for
/// webcams; MJPEG uses JFIF full range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Layout {
    Yuyv,
    Uyvy,
    Yvyu,
    Nv12,
    Nv21,
    I420,
    Yv12,
    Rgb24,
    Bgr24,
    /// Four bytes per pixel in B, G, R, X order (AVFoundation BGRA, V4L2 XR24/AR24, MF RGB32).
    Bgrx,
    Gray,
    Mjpeg,
}

impl Layout {
    /// Mode-selection tie-breaker: decoding JPEG costs more than converting raw pixels.
    pub(crate) fn decode_cost(self) -> f64 {
        match self {
            Self::Mjpeg => crate::modes::COMPRESSED,
            _ => 0.0,
        }
    }
}

/// One native frame as delivered by a backend.
pub(crate) struct RawFrame<'a> {
    pub layout: Layout,
    pub width: usize,
    pub height: usize,
    /// Bytes per row of the first plane; zero means tightly packed.
    pub stride: usize,
    /// Rows are stored bottom row first (Windows DIB-style RGB).
    pub bottom_up: bool,
    /// YUV and grey samples use 0-255 instead of 16-235 (luma) / 16-240 (chroma).
    pub full_range: bool,
    pub data: &'a [u8],
}

/// A converted frame: width, height and packed RGB bytes.
pub(crate) type Rgb = (u32, u32, Vec<u8>);

/// Convert a native frame to packed RGB.
pub(crate) fn to_rgb(frame: &RawFrame<'_>) -> Result<Rgb, String> {
    let (width, height) = (frame.width, frame.height);
    if width == 0 || height == 0 {
        return Err("frame has no pixels".into());
    }
    if frame.layout == Layout::Mjpeg {
        let image = jpeg::decode(frame.data)?;
        return Ok((image.width, image.height, image.rgb));
    }

    // Returns (stride, bytes read per row) for a packed layout with `row_len` bytes per row.
    let packed = |row_len: usize| {
        let stride = if frame.stride == 0 {
            row_len
        } else {
            frame.stride
        };
        (stride, row_len)
    };
    let mut rgb = vec![0u8; width * height * 3];
    match frame.layout {
        Layout::Yuyv | Layout::Uyvy | Layout::Yvyu => {
            // Odd widths still store a whole final two-pixel group.
            let (stride, row_len) = packed(width.div_ceil(2) * 4);
            // Byte offsets of Y0, U, Y1, V within each two-pixel group.
            let (y0, u, y1, v) = match frame.layout {
                Layout::Yuyv => (0, 1, 2, 3),
                Layout::Uyvy => (1, 0, 3, 2),
                _ => (0, 3, 2, 1),
            };
            for (y, out) in rgb.chunks_exact_mut(width * 3).enumerate() {
                let row = row(frame, stride, row_len, y)?;
                for (x, px) in out.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let group = &row[(x / 2) * 4..(x / 2) * 4 + 4];
                    let luma = if x % 2 == 0 { group[y0] } else { group[y1] };
                    yuv(px, luma, group[u], group[v], frame.full_range);
                }
            }
        }
        Layout::Nv12 | Layout::Nv21 | Layout::I420 | Layout::Yv12 => {
            let stride = if frame.stride == 0 {
                width
            } else {
                frame.stride
            };
            let (chroma_w, chroma_h) = (width.div_ceil(2), height.div_ceil(2));
            let luma_len = stride * height;
            let semi = matches!(frame.layout, Layout::Nv12 | Layout::Nv21);
            let chroma_stride = if semi { stride } else { stride / 2 };
            let chroma_len = chroma_stride * chroma_h;
            let needed = if semi {
                luma_len + chroma_len
            } else {
                luma_len + chroma_len * 2
            };
            if frame.data.len() < needed
                || stride < width
                || chroma_stride < if semi { chroma_w * 2 } else { chroma_w }
            {
                return Err(format!(
                    "{:?} frame is {} bytes, expected at least {needed}",
                    frame.layout,
                    frame.data.len()
                ));
            }
            let (luma, chroma) = frame.data.split_at(luma_len);
            for (y, out) in rgb.chunks_exact_mut(width * 3).enumerate() {
                let luma_row = &luma[y * stride..];
                let c = (y / 2) * chroma_stride;
                for (x, px) in out.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let (cb, cr) = match frame.layout {
                        Layout::Nv12 => (chroma[c + x / 2 * 2], chroma[c + x / 2 * 2 + 1]),
                        Layout::Nv21 => (chroma[c + x / 2 * 2 + 1], chroma[c + x / 2 * 2]),
                        Layout::I420 => (chroma[c + x / 2], chroma[chroma_len + c + x / 2]),
                        _ => (chroma[chroma_len + c + x / 2], chroma[c + x / 2]),
                    };
                    yuv(px, luma_row[x], cb, cr, frame.full_range);
                }
            }
        }
        Layout::Rgb24 | Layout::Bgr24 | Layout::Bgrx | Layout::Gray => {
            let bytes = match frame.layout {
                Layout::Bgrx => 4,
                Layout::Gray => 1,
                _ => 3,
            };
            let (stride, row_len) = packed(width * bytes);
            for (y, out) in rgb.chunks_exact_mut(width * 3).enumerate() {
                let row = row(frame, stride, row_len, y)?;
                match frame.layout {
                    Layout::Rgb24 => out.copy_from_slice(row),
                    Layout::Gray => {
                        for (px, &g) in out.as_chunks_mut::<3>().0.iter_mut().zip(row) {
                            px.fill(if frame.full_range {
                                g
                            } else {
                                clamp(((i32::from(g) - 16) * 255 + 109) / 219)
                            });
                        }
                    }
                    _ => {
                        for (px, src) in out
                            .as_chunks_mut::<3>()
                            .0
                            .iter_mut()
                            .zip(row.chunks_exact(bytes))
                        {
                            px[0] = src[2];
                            px[1] = src[1];
                            px[2] = src[0];
                        }
                    }
                }
            }
        }
        Layout::Mjpeg => unreachable!("handled above"),
    }
    Ok((width as u32, height as u32, rgb))
}

fn row<'a>(
    frame: &RawFrame<'a>,
    stride: usize,
    row_len: usize,
    y: usize,
) -> Result<&'a [u8], String> {
    let source_row = if frame.bottom_up {
        frame.height - 1 - y
    } else {
        y
    };
    let start = source_row * stride;
    frame
        .data
        .get(start..start + row_len)
        .filter(|_| stride >= row_len)
        .ok_or_else(|| {
            format!(
                "{:?} frame is {} bytes, too short for {}x{} with stride {stride}",
                frame.layout,
                frame.data.len(),
                frame.width,
                frame.height
            )
        })
}

/// BT.601 Y'CbCr to RGB, in limited or full (JFIF) range.
pub(crate) fn yuv(px: &mut [u8], y: u8, u: u8, v: u8, full_range: bool) {
    if full_range {
        let luma = i32::from(y) << 16;
        let cb = i32::from(u) - 128;
        let cr = i32::from(v) - 128;
        // JFIF coefficients in 16.16 fixed point.
        px[0] = clamp((luma + 91881 * cr + 32768) >> 16);
        px[1] = clamp((luma - 22554 * cb - 46802 * cr + 32768) >> 16);
        px[2] = clamp((luma + 116130 * cb + 32768) >> 16);
        return;
    }
    let c = (i32::from(y) - 16) * 298;
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    px[0] = clamp((c + 409 * e + 128) >> 8);
    px[1] = clamp((c - 100 * d - 208 * e + 128) >> 8);
    px[2] = clamp((c + 516 * d + 128) >> 8);
}

fn clamp(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(layout: Layout, width: usize, height: usize, stride: usize, data: &[u8]) -> Vec<u8> {
        to_rgb(&RawFrame {
            layout,
            width,
            height,
            stride,
            bottom_up: false,
            full_range: false,
            data,
        })
        .unwrap()
        .2
    }

    // Limited-range white, black and pure red (Y, U, V).
    const WHITE: [u8; 3] = [235, 128, 128];
    const BLACK: [u8; 3] = [16, 128, 128];
    const RED: [u8; 3] = [81, 90, 240];

    fn close(actual: &[u8], expected: &[u8]) {
        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected) {
            assert!(a.abs_diff(*e) <= 2, "{actual:?} != {expected:?}");
        }
    }

    #[test]
    fn packed_yuv_orders_decode_the_same_pixels() {
        let [wy, wu, wv] = WHITE;
        let [by, ..] = BLACK;
        let expected = [255, 255, 255, 0, 0, 0];
        close(
            &convert(Layout::Yuyv, 2, 1, 0, &[wy, wu, by, wv]),
            &expected,
        );
        close(
            &convert(Layout::Uyvy, 2, 1, 0, &[wu, wy, wv, by]),
            &expected,
        );
        close(
            &convert(Layout::Yvyu, 2, 1, 0, &[wy, wv, by, wu]),
            &expected,
        );
        let [ry, ru, rv] = RED;
        close(
            &convert(Layout::Yuyv, 2, 1, 0, &[ry, ru, ry, rv]),
            &[255, 0, 0, 255, 0, 0],
        );
    }

    #[test]
    fn planar_yuv_layouts_respect_plane_order_and_stride() {
        let [ry, ru, rv] = RED;
        // 2x2 red frame with 4 bytes of luma stride (2 bytes padding per row).
        let luma = [ry, ry, 0, 0, ry, ry, 0, 0];
        let red = [255, 0, 0].repeat(4);
        close(
            &convert(
                Layout::Nv12,
                2,
                2,
                4,
                &[&luma[..], &[ru, rv, 0, 0]].concat(),
            ),
            &red,
        );
        close(
            &convert(
                Layout::Nv21,
                2,
                2,
                4,
                &[&luma[..], &[rv, ru, 0, 0]].concat(),
            ),
            &red,
        );
        close(
            &convert(
                Layout::I420,
                2,
                2,
                4,
                &[&luma[..], &[ru, 0, rv, 0]].concat(),
            ),
            &red,
        );
        close(
            &convert(
                Layout::Yv12,
                2,
                2,
                4,
                &[&luma[..], &[rv, 0, ru, 0]].concat(),
            ),
            &red,
        );
    }

    #[test]
    fn rgb_layouts_and_bottom_up_rows() {
        assert_eq!(convert(Layout::Bgr24, 1, 1, 0, &[3, 2, 1]), [1, 2, 3]);
        assert_eq!(convert(Layout::Bgrx, 1, 1, 0, &[3, 2, 1, 0]), [1, 2, 3]);
        assert_eq!(
            convert(Layout::Gray, 2, 1, 0, &[16, 235]),
            [0, 0, 0, 255, 255, 255]
        );
        let flipped = to_rgb(&RawFrame {
            layout: Layout::Rgb24,
            width: 1,
            height: 2,
            stride: 4,
            bottom_up: true,
            full_range: false,
            data: &[1, 1, 1, 0, 2, 2, 2, 0],
        })
        .unwrap();
        assert_eq!(flipped.2, [2, 2, 2, 1, 1, 1]);
    }

    #[test]
    fn short_frames_are_errors_not_panics() {
        for layout in [
            Layout::Yuyv,
            Layout::Nv12,
            Layout::I420,
            Layout::Bgrx,
            Layout::Mjpeg,
        ] {
            assert!(
                to_rgb(&RawFrame {
                    layout,
                    width: 64,
                    height: 64,
                    stride: 0,
                    bottom_up: false,
                    full_range: false,
                    data: &[0; 100],
                })
                .is_err()
            );
        }
    }
}
