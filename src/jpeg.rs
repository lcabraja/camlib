//! Baseline JPEG decoding for MJPEG cameras.
//!
//! UVC cameras send each frame as a baseline (sequential Huffman, 8-bit) JPEG, usually without
//! Huffman tables, which then default to the standard tables from ITU T.81 Annex K. Progressive,
//! arithmetic-coded and 12-bit JPEGs are rejected. Chroma is upsampled by replication, and the
//! inverse DCT is the IJG "islow" integer transform.

use crate::convert::yuv;

pub(crate) struct Image {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Largest frame accepted, bounding allocations made from a corrupt header.
const MAX_PIXELS: usize = 8192 * 8192;

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const AC_LUMA_VALUES: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];
const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALUES: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
    0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

const FAST_BITS: u32 = 9;

struct Huffman {
    /// For codes of up to `FAST_BITS` bits: `length << 8 | value`, or 0 for longer codes.
    fast: Vec<u16>,
    max_code: [i32; 17],
    min_code: [i32; 17],
    first_index: [usize; 17],
    values: Vec<u8>,
}

impl Huffman {
    fn new(bits: &[u8; 16], values: &[u8]) -> Result<Self, String> {
        let total: usize = bits.iter().map(|&b| usize::from(b)).sum();
        if total > values.len() || total > 256 {
            return Err("JPEG Huffman table is malformed".into());
        }
        let mut table = Self {
            fast: vec![0; 1 << FAST_BITS],
            max_code: [-1; 17],
            min_code: [0; 17],
            first_index: [0; 17],
            values: values[..total].to_vec(),
        };
        let (mut code, mut index) = (0i32, 0usize);
        for length in 1..=16u32 {
            let count = usize::from(bits[length as usize - 1]);
            table.first_index[length as usize] = index;
            table.min_code[length as usize] = code;
            for _ in 0..count {
                if code >= 1 << length {
                    return Err("JPEG Huffman table has too many codes".into());
                }
                if length <= FAST_BITS {
                    let shift = FAST_BITS - length;
                    let start = (code as usize) << shift;
                    let entry = (length as u16) << 8 | u16::from(table.values[index]);
                    table.fast[start..start + (1 << shift)].fill(entry);
                }
                code += 1;
                index += 1;
            }
            if count > 0 {
                table.max_code[length as usize] = code - 1;
            }
            code <<= 1;
        }
        Ok(table)
    }
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    /// Pending bits, most significant first.
    acc: u64,
    count: u32,
    /// Set once a marker interrupts entropy-coded data; zeros are fed after it.
    marker: bool,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos,
            acc: 0,
            count: 0,
            marker: false,
        }
    }

    fn fill(&mut self) {
        while self.count <= 56 {
            let mut byte = 0;
            if !self.marker && self.pos < self.data.len() {
                byte = self.data[self.pos];
                if byte == 0xff {
                    match self.data.get(self.pos + 1) {
                        Some(0) => self.pos += 2,
                        _ => {
                            self.marker = true;
                            byte = 0;
                        }
                    }
                } else {
                    self.pos += 1;
                }
            }
            self.acc |= u64::from(byte) << (56 - self.count);
            self.count += 8;
        }
    }

    fn consume(&mut self, bits: u32) {
        self.acc <<= bits;
        self.count -= bits;
    }

    fn huffman(&mut self, table: &Huffman) -> Result<u8, String> {
        self.fill();
        let entry = table.fast[(self.acc >> (64 - FAST_BITS)) as usize];
        if entry != 0 {
            self.consume(u32::from(entry >> 8));
            return Ok(entry as u8);
        }
        for length in FAST_BITS + 1..=16 {
            let code = (self.acc >> (64 - length)) as i32;
            if code <= table.max_code[length as usize] {
                self.consume(length);
                let index = table.first_index[length as usize]
                    + (code - table.min_code[length as usize]) as usize;
                return table
                    .values
                    .get(index)
                    .copied()
                    .ok_or_else(|| "JPEG Huffman code is out of range".into());
            }
        }
        Err("JPEG data has an invalid Huffman code".into())
    }

    /// Read `size` bits as a signed coefficient (T.81 F.2.2.1 EXTEND).
    fn signed(&mut self, size: u8) -> i32 {
        if size == 0 {
            return 0;
        }
        let size = u32::from(size.min(16));
        self.fill();
        let value = (self.acc >> (64 - size)) as i32;
        self.consume(size);
        if value < 1 << (size - 1) {
            value - (1 << size) + 1
        } else {
            value
        }
    }

    /// Discard buffered bits and step over the restart marker that ends this interval.
    fn restart(&mut self) {
        self.acc = 0;
        self.count = 0;
        self.marker = false;
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xff && (0xd0..=0xd7).contains(&self.data[self.pos + 1]) {
                self.pos += 2;
                return;
            }
            self.pos += 1;
        }
    }
}

struct Component {
    id: u8,
    h: usize,
    v: usize,
    quant: usize,
    /// Decoded samples, padded to whole MCUs.
    plane: Vec<u8>,
    stride: usize,
    dc_pred: i32,
}

pub(crate) fn decode(data: &[u8]) -> Result<Image, String> {
    if data.get(..2) != Some(&[0xff, 0xd8]) {
        return Err("MJPEG frame does not start with a JPEG marker".into());
    }
    let mut quant = [[0u16; 64]; 4];
    let mut dc: [Option<Huffman>; 4] = Default::default();
    let mut ac: [Option<Huffman>; 4] = Default::default();
    dc[0] = Some(Huffman::new(&DC_LUMA_BITS, &DC_VALUES)?);
    dc[1] = Some(Huffman::new(&DC_CHROMA_BITS, &DC_VALUES)?);
    ac[0] = Some(Huffman::new(&AC_LUMA_BITS, &AC_LUMA_VALUES)?);
    ac[1] = Some(Huffman::new(&AC_CHROMA_BITS, &AC_CHROMA_VALUES)?);
    let mut restart_interval = 0usize;
    let mut frame: Option<(usize, usize, Vec<Component>)> = None;
    let mut pos = 2;

    loop {
        // Find the next marker, skipping fill bytes and stray data.
        while pos < data.len() && data[pos] != 0xff {
            pos += 1;
        }
        while pos < data.len() && data[pos] == 0xff {
            pos += 1;
        }
        let Some(&marker) = data.get(pos) else {
            break;
        };
        pos += 1;
        match marker {
            0xd9 => break,
            0xd8 | 0xd0..=0xd7 | 0x01 => continue,
            _ => {}
        }
        let length = usize::from(u16::from_be_bytes([
            *data.get(pos).ok_or("JPEG segment is truncated")?,
            *data.get(pos + 1).ok_or("JPEG segment is truncated")?,
        ]));
        let segment = data
            .get(pos + 2..pos + length)
            .filter(|_| length >= 2)
            .ok_or("JPEG segment is truncated")?;
        pos += length;
        match marker {
            0xc0 | 0xc1 => frame = Some(read_frame(segment)?),
            0xc2..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                return Err("only baseline JPEG is supported for MJPEG".into());
            }
            0xc4 => read_huffman(segment, &mut dc, &mut ac)?,
            0xdb => read_quant(segment, &mut quant)?,
            0xdd => {
                let value = segment
                    .get(..2)
                    .ok_or("JPEG restart interval is truncated")?;
                restart_interval = usize::from(u16::from_be_bytes([value[0], value[1]]));
            }
            0xda => {
                let (width, height, components) = frame
                    .as_mut()
                    .ok_or("JPEG scan precedes its frame header")?;
                pos = decode_scan(
                    data,
                    pos,
                    segment,
                    (*width, *height),
                    components,
                    &quant,
                    (&dc, &ac),
                    restart_interval,
                )?;
            }
            _ => {}
        }
    }

    let (width, height, components) = frame.ok_or("JPEG has no frame header")?;
    Ok(Image {
        width: width as u32,
        height: height as u32,
        rgb: to_rgb(width, height, &components)?,
    })
}

fn read_frame(segment: &[u8]) -> Result<(usize, usize, Vec<Component>), String> {
    let header = segment.get(..6).ok_or("JPEG frame header is truncated")?;
    if header[0] != 8 {
        return Err(format!("{}-bit JPEG is not supported", header[0]));
    }
    let height = usize::from(u16::from_be_bytes([header[1], header[2]]));
    let width = usize::from(u16::from_be_bytes([header[3], header[4]]));
    let count = usize::from(header[5]);
    if width == 0 || height == 0 || width * height > MAX_PIXELS {
        return Err(format!("JPEG size {width}x{height} is not supported"));
    }
    if count != 1 && count != 3 {
        return Err(format!("JPEG with {count} components is not supported"));
    }
    let mut components = Vec::with_capacity(count);
    for c in segment[6..].as_chunks::<3>().0.iter().take(count) {
        let (h, v) = (usize::from(c[1] >> 4), usize::from(c[1] & 15));
        if !(1..=4).contains(&h) || !(1..=4).contains(&v) || c[2] > 3 {
            return Err("JPEG component header is invalid".into());
        }
        components.push(Component {
            id: c[0],
            h,
            v,
            quant: usize::from(c[2]),
            plane: Vec::new(),
            stride: 0,
            dc_pred: 0,
        });
    }
    if components.len() != count {
        return Err("JPEG frame header is truncated".into());
    }
    let h_max = components.iter().map(|c| c.h).max().unwrap_or(1);
    let v_max = components.iter().map(|c| c.v).max().unwrap_or(1);
    let (mcus_x, mcus_y) = (width.div_ceil(8 * h_max), height.div_ceil(8 * v_max));
    for c in &mut components {
        c.stride = mcus_x * c.h * 8;
        c.plane = vec![0; c.stride * mcus_y * c.v * 8];
    }
    Ok((width, height, components))
}

fn read_huffman(
    mut segment: &[u8],
    dc: &mut [Option<Huffman>; 4],
    ac: &mut [Option<Huffman>; 4],
) -> Result<(), String> {
    while !segment.is_empty() {
        let header = segment.get(..17).ok_or("JPEG Huffman table is truncated")?;
        let (class, slot) = (header[0] >> 4, usize::from(header[0] & 15));
        let bits: [u8; 16] = header[1..17].try_into().unwrap_or_default();
        let total: usize = bits.iter().map(|&b| usize::from(b)).sum();
        let values = segment
            .get(17..17 + total)
            .ok_or("JPEG Huffman table is truncated")?;
        let table = Some(Huffman::new(&bits, values)?);
        match (class, slot) {
            (0, 0..=3) => dc[slot] = table,
            (1, 0..=3) => ac[slot] = table,
            _ => return Err("JPEG Huffman table id is invalid".into()),
        }
        segment = &segment[17 + total..];
    }
    Ok(())
}

fn read_quant(mut segment: &[u8], quant: &mut [[u16; 64]; 4]) -> Result<(), String> {
    while let Some(&info) = segment.first() {
        let (precision, slot) = (info >> 4, usize::from(info & 15));
        let size = if precision == 0 { 64 } else { 128 };
        let values = segment
            .get(1..1 + size)
            .ok_or("JPEG quantization table is truncated")?;
        let table = quant
            .get_mut(slot)
            .ok_or("JPEG quantization table id is invalid")?;
        for (k, q) in table.iter_mut().enumerate() {
            *q = if precision == 0 {
                u16::from(values[k])
            } else {
                u16::from_be_bytes([values[2 * k], values[2 * k + 1]])
            };
        }
        segment = &segment[1 + size..];
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn decode_scan(
    data: &[u8],
    start: usize,
    header: &[u8],
    (width, height): (usize, usize),
    components: &mut [Component],
    quant: &[[u16; 64]; 4],
    (dc, ac): (&[Option<Huffman>; 4], &[Option<Huffman>; 4]),
    restart_interval: usize,
) -> Result<usize, String> {
    let count = usize::from(*header.first().ok_or("JPEG scan header is truncated")?);
    let mut scan = Vec::with_capacity(count);
    for selector in header
        .get(1..1 + 2 * count)
        .ok_or("JPEG scan header is truncated")?
        .as_chunks::<2>()
        .0
        .iter()
    {
        let index = components
            .iter()
            .position(|c| c.id == selector[0])
            .ok_or("JPEG scan names an unknown component")?;
        let dc_table = dc[usize::from(selector[1] >> 4) & 3]
            .as_ref()
            .ok_or("JPEG scan uses a missing DC table")?;
        let ac_table = ac[usize::from(selector[1] & 15) & 3]
            .as_ref()
            .ok_or("JPEG scan uses a missing AC table")?;
        scan.push((index, dc_table, ac_table));
    }
    let h_max = components.iter().map(|c| c.h).max().unwrap_or(1);
    let v_max = components.iter().map(|c| c.v).max().unwrap_or(1);
    for c in components.iter_mut() {
        c.dc_pred = 0;
    }

    // An interleaved scan walks MCUs; a single-component scan walks that component's blocks.
    let (units_x, units_y) = if scan.len() == 1 {
        let c = &components[scan[0].0];
        (
            (width * c.h).div_ceil(h_max).div_ceil(8),
            (height * c.v).div_ceil(v_max).div_ceil(8),
        )
    } else {
        (width.div_ceil(8 * h_max), height.div_ceil(8 * v_max))
    };

    let mut bits = Bits::new(data, start);
    let mut coefficients = [0i32; 64];
    let mut until_restart = restart_interval;
    for unit_y in 0..units_y {
        for unit_x in 0..units_x {
            if restart_interval > 0 {
                if until_restart == 0 {
                    bits.restart();
                    for c in components.iter_mut() {
                        c.dc_pred = 0;
                    }
                    until_restart = restart_interval;
                }
                until_restart -= 1;
            }
            for &(index, dc_table, ac_table) in &scan {
                let component = &mut components[index];
                let table = &quant[component.quant];
                let (blocks_h, blocks_v) = if scan.len() == 1 {
                    (1, 1)
                } else {
                    (component.h, component.v)
                };
                for v in 0..blocks_v {
                    for h in 0..blocks_h {
                        coefficients.fill(0);
                        let size = bits.huffman(dc_table)?;
                        component.dc_pred = component.dc_pred.wrapping_add(bits.signed(size));
                        coefficients[0] = dequantize(component.dc_pred, table[0]);
                        let mut k = 1;
                        while k < 64 {
                            let rs = bits.huffman(ac_table)?;
                            let (run, size) = (usize::from(rs >> 4), rs & 15);
                            if size == 0 {
                                if run != 15 {
                                    break;
                                }
                                k += 16;
                                continue;
                            }
                            k += run;
                            if k > 63 {
                                return Err("JPEG block has too many coefficients".into());
                            }
                            coefficients[ZIGZAG[k]] = dequantize(bits.signed(size), table[k]);
                            k += 1;
                        }
                        let (block_x, block_y) = if scan.len() == 1 {
                            (unit_x, unit_y)
                        } else {
                            (unit_x * component.h + h, unit_y * component.v + v)
                        };
                        let offset = block_y * 8 * component.stride + block_x * 8;
                        if offset + 7 * component.stride + 8 <= component.plane.len() {
                            idct(
                                &coefficients,
                                &mut component.plane[offset..],
                                component.stride,
                            );
                        }
                    }
                }
            }
        }
    }

    // Resume marker parsing at the first marker that is not a restart.
    let mut pos = bits.pos;
    while pos + 1 < data.len() && (data[pos] != 0xff || matches!(data[pos + 1], 0x00 | 0xd0..=0xd7))
    {
        pos += 1;
    }
    Ok(pos)
}

/// Valid 8-bit JPEG coefficients stay far inside this range; clamping keeps corrupt frames from
/// overflowing the IDCT.
fn dequantize(value: i32, quant: u16) -> i32 {
    value.saturating_mul(i32::from(quant)).clamp(-32768, 32767)
}

/// Upsample one component to full resolution. 2x horizontal (4:2:2) and 2x2 (4:2:0) chroma use
/// libjpeg's "fancy" triangle filter, matching its output; other ratios replicate samples.
fn upsample(c: &Component, width: usize, height: usize, h_max: usize, v_max: usize) -> Vec<u8> {
    // Size of the component's real (unpadded) samples.
    let (cw, ch) = (
        (width * c.h).div_ceil(h_max),
        (height * c.v).div_ceil(v_max),
    );
    let sample = |x: usize, y: usize| u32::from(c.plane[y * c.stride + x]);
    let mut out = vec![0u8; width * height];
    let fancy = c.h * 2 == h_max && (c.v == v_max || c.v * 2 == v_max) && cw >= 2;
    if !fancy {
        for (y, row) in out.chunks_exact_mut(width).enumerate() {
            let sy = y * c.v / v_max;
            for (x, value) in row.iter_mut().enumerate() {
                *value = c.plane[sy * c.stride + x * c.h / h_max];
            }
        }
        return out;
    }
    let vertical = c.v * 2 == v_max;
    let mut wide = vec![0u8; cw * 2];
    for (y, row) in out.chunks_exact_mut(width).enumerate() {
        if vertical {
            // Weight the nearer input row 3:1 against the next row away (h2v2_fancy_upsample).
            let near = (y / 2).min(ch - 1);
            let far = if y % 2 == 0 {
                near.saturating_sub(1)
            } else {
                (near + 1).min(ch - 1)
            };
            let column = |x: usize| sample(x, near) * 3 + sample(x, far);
            let (mut last, mut this) = (column(0), column(0));
            for x in 0..cw {
                let next = if x + 1 < cw { column(x + 1) } else { this };
                wide[2 * x] = if x == 0 {
                    (this * 4 + 8) >> 4
                } else {
                    (this * 3 + last + 8) >> 4
                } as u8;
                wide[2 * x + 1] = if x + 1 == cw {
                    (this * 4 + 7) >> 4
                } else {
                    (this * 3 + next + 7) >> 4
                } as u8;
                last = this;
                this = next;
            }
        } else {
            // h2v1_fancy_upsample.
            let sy = y * c.v / v_max;
            for x in 0..cw {
                let this = sample(x, sy) * 3;
                wide[2 * x] = if x == 0 {
                    sample(0, sy)
                } else {
                    (this + sample(x - 1, sy) + 1) >> 2
                } as u8;
                wide[2 * x + 1] = if x + 1 == cw {
                    sample(x, sy)
                } else {
                    (this + sample(x + 1, sy) + 2) >> 2
                } as u8;
            }
        }
        row.copy_from_slice(&wide[..width]);
    }
    out
}

fn to_rgb(width: usize, height: usize, components: &[Component]) -> Result<Vec<u8>, String> {
    let h_max = components.iter().map(|c| c.h).max().unwrap_or(1);
    let v_max = components.iter().map(|c| c.v).max().unwrap_or(1);
    let planes: Vec<Vec<u8>> = components
        .iter()
        .map(|c| upsample(c, width, height, h_max, v_max))
        .collect();
    if planes.len() == 1 {
        return Ok(planes[0].iter().flat_map(|&g| [g, g, g]).collect());
    }
    let mut rgb = vec![0u8; width * height * 3];
    for (i, px) in rgb.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        yuv(px, planes[0][i], planes[1][i], planes[2][i], true);
    }
    Ok(rgb)
}

const CONST_BITS: i64 = 13;
const PASS1_BITS: i64 = 2;
const FIX_0_298631336: i64 = 2446;
const FIX_0_390180644: i64 = 3196;
const FIX_0_541196100: i64 = 4433;
const FIX_0_765366865: i64 = 6270;
const FIX_0_899976223: i64 = 7373;
const FIX_1_175875602: i64 = 9633;
const FIX_1_501321110: i64 = 12299;
const FIX_1_847759065: i64 = 15137;
const FIX_1_961570560: i64 = 16069;
const FIX_2_053119869: i64 = 16819;
const FIX_2_562915447: i64 = 20995;
const FIX_3_072711026: i64 = 25172;

/// One 1-D pass of the IJG islow IDCT over eight values `step` apart.
fn idct_1d(v: [i64; 8]) -> [i64; 8] {
    let z1 = (v[2] + v[6]) * FIX_0_541196100;
    let tmp2 = z1 - v[6] * FIX_1_847759065;
    let tmp3 = z1 + v[2] * FIX_0_765366865;
    let tmp0 = (v[0] + v[4]) << CONST_BITS;
    let tmp1 = (v[0] - v[4]) << CONST_BITS;
    let (tmp10, tmp13, tmp11, tmp12) = (tmp0 + tmp3, tmp0 - tmp3, tmp1 + tmp2, tmp1 - tmp2);

    let (mut o0, mut o1, mut o2, mut o3) = (v[7], v[5], v[3], v[1]);
    let z1 = o0 + o3;
    let z2 = o1 + o2;
    let z3 = o0 + o2;
    let z4 = o1 + o3;
    let z5 = (z3 + z4) * FIX_1_175875602;
    o0 *= FIX_0_298631336;
    o1 *= FIX_2_053119869;
    o2 *= FIX_3_072711026;
    o3 *= FIX_1_501321110;
    let z1 = -z1 * FIX_0_899976223;
    let z2 = -z2 * FIX_2_562915447;
    let z3 = -z3 * FIX_1_961570560 + z5;
    let z4 = -z4 * FIX_0_390180644 + z5;
    o0 += z1 + z3;
    o1 += z2 + z4;
    o2 += z2 + z3;
    o3 += z1 + z4;
    [
        tmp10 + o3,
        tmp11 + o2,
        tmp12 + o1,
        tmp13 + o0,
        tmp13 - o0,
        tmp12 - o1,
        tmp11 - o2,
        tmp10 - o3,
    ]
}

fn descale(value: i64, shift: i64) -> i64 {
    (value + (1 << (shift - 1))) >> shift
}

/// Inverse-DCT one dequantized block into 8x8 level-shifted samples.
fn idct(coefficients: &[i32; 64], out: &mut [u8], stride: usize) {
    // 64-bit intermediates cannot overflow, even for clamped garbage coefficients.
    let mut work = [0i64; 64];
    for x in 0..8 {
        let column: [i64; 8] = std::array::from_fn(|y| i64::from(coefficients[y * 8 + x]));
        if column[1..].iter().all(|&c| c == 0) {
            let dc = column[0] << PASS1_BITS;
            for y in 0..8 {
                work[y * 8 + x] = dc;
            }
            continue;
        }
        for (y, value) in idct_1d(column).into_iter().enumerate() {
            work[y * 8 + x] = descale(value, CONST_BITS - PASS1_BITS);
        }
    }
    for y in 0..8 {
        let row: [i64; 8] = work[y * 8..y * 8 + 8].try_into().unwrap_or_default();
        for (x, value) in idct_1d(row).into_iter().enumerate() {
            let sample = descale(value, CONST_BITS + PASS1_BITS + 3) + 128;
            out[y * stride + x] = sample.clamp(0, 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_huffman_tables_are_complete_prefix_codes() {
        for (bits, values) in [
            (&DC_LUMA_BITS, &DC_VALUES[..]),
            (&DC_CHROMA_BITS, &DC_VALUES[..]),
            (&AC_LUMA_BITS, &AC_LUMA_VALUES[..]),
            (&AC_CHROMA_BITS, &AC_CHROMA_VALUES[..]),
        ] {
            let total: usize = bits.iter().map(|&b| usize::from(b)).sum();
            assert_eq!(total, values.len());
            Huffman::new(bits, values).unwrap();
        }
    }

    #[test]
    fn integer_idct_matches_the_float_definition() {
        let mut seed = 0x2545_f491_u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for _ in 0..200 {
            let mut coefficients = [0i32; 64];
            for c in coefficients.iter_mut().take(20) {
                *c = (next() % 201) as i32 - 100;
            }
            coefficients[0] = (next() % 1001) as i32 - 500;
            let mut out = [0u8; 64];
            idct(&coefficients, &mut out, 8);
            for y in 0..8 {
                for x in 0..8 {
                    let mut sum = 0.0;
                    for v in 0..8 {
                        for u in 0..8 {
                            let cu = if u == 0 { 1.0 / 2f64.sqrt() } else { 1.0 };
                            let cv = if v == 0 { 1.0 / 2f64.sqrt() } else { 1.0 };
                            sum += cu
                                * cv
                                * f64::from(coefficients[v * 8 + u])
                                * (((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI) / 16.0)
                                    .cos()
                                * (((2 * y + 1) as f64 * v as f64 * std::f64::consts::PI) / 16.0)
                                    .cos();
                        }
                    }
                    let expected = (sum / 4.0 + 128.0).round().clamp(0.0, 255.0);
                    let actual = f64::from(out[y * 8 + x]);
                    assert!((actual - expected).abs() <= 1.0, "{actual} vs {expected}");
                }
            }
        }
    }

    /// Fixtures encoded by Pillow (libjpeg) and ffmpeg, with libjpeg's own decode as reference.
    /// `-nodht` files drop their Huffman tables like UVC cameras do.
    #[test]
    fn decodes_like_libjpeg() {
        macro_rules! fixture {
            ($name:literal, $max_mean:expr) => {
                (
                    $name,
                    &include_bytes!(concat!("../tests/fixtures/", $name, ".jpg"))[..],
                    &include_bytes!(concat!("../tests/fixtures/", $name, ".rgb"))[..],
                    $max_mean,
                )
            };
        }
        // Same IDCT, upsampling and color conversion as libjpeg: the output is bit-exact.
        for (name, jpeg, reference, max_mean) in [
            fixture!("pil-444", 0.0),
            fixture!("pil-gray", 0.0),
            fixture!("pil-422", 0.0),
            fixture!("pil-420", 0.0),
            fixture!("pil-420-odd", 0.0),
            fixture!("pil-420-restart", 0.0),
            fixture!("pil-420-nodht", 0.0),
            fixture!("ffmpeg-mjpeg-422", 0.0),
            fixture!("ffmpeg-mjpeg-420", 0.0),
        ] {
            let image = decode(jpeg).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(image.rgb.len(), reference.len(), "{name}");
            let total: u64 = image
                .rgb
                .iter()
                .zip(reference)
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum();
            let mean = total as f64 / reference.len() as f64;
            eprintln!(
                "{name}: {}x{} mean error {mean:.3}",
                image.width, image.height
            );
            assert!(mean <= max_mean, "{name}: mean error {mean:.3}");
        }
    }

    #[test]
    fn truncated_frames_never_panic() {
        let jpeg = include_bytes!("../tests/fixtures/ffmpeg-mjpeg-422.jpg");
        for end in (0..jpeg.len()).step_by(97) {
            let _ = decode(&jpeg[..end]);
        }
        let mut corrupt = jpeg.to_vec();
        for i in (700..corrupt.len()).step_by(13) {
            corrupt[i] ^= 0x5a;
        }
        let _ = decode(&corrupt);
    }

    #[test]
    fn corrupt_input_is_an_error() {
        assert!(decode(&[]).is_err());
        assert!(decode(&[0xff, 0xd8, 0xff, 0xd9]).is_err());
        assert!(decode(&[0xff, 0xd8, 0xff, 0xc0, 0x00, 0x11, 8]).is_err());
    }
}
