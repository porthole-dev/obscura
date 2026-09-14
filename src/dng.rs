// SPDX-License-Identifier: GPL-3.0-or-later
//! A minimal DNG writer: one uncompressed CFA strip, with the colour and
//! exposure metadata a raw developer needs. The colour maths follows
//! libcamera's own src/apps/common/dng_writer.cpp.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};

use crate::camera::{RawImage, Still};

struct Layout {
    cfa: [u8; 4],
    bits: u32,
    packed: bool,
}

/// "SRGGB10_CSI2P" -> RGGB, 10 bits, CSI-2 packed.
fn layout(format: &str) -> Option<Layout> {
    let name = format.strip_prefix('S')?;
    let cfa = match name.get(..4)? {
        "RGGB" => [0, 1, 1, 2],
        "GRBG" => [1, 0, 2, 1],
        "GBRG" => [1, 2, 0, 1],
        "BGGR" => [2, 1, 1, 0],
        _ => return None,
    };
    let rest = &name[4..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    Some(Layout { cfa, bits: digits.parse().ok()?, packed: rest.contains("CSI2P") })
}

fn unpack(raw: &RawImage, l: &Layout) -> Result<Vec<u16>> {
    let (w, h, stride) = (raw.width as usize, raw.height as usize, raw.stride as usize);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let Some(row) = raw.data.get(y * stride..y * stride + stride.min(raw.data.len() - y * stride)) else {
            bail!("raw frame is short at row {y}");
        };
        match (l.bits, l.packed) {
            (8, _) => out.extend(row[..w].iter().map(|b| *b as u16)),
            (10, true) => {
                for x in 0..w {
                    let group = &row[x / 4 * 5..];
                    let i = x % 4;
                    out.push(((group[i] as u16) << 2) | ((group[4] as u16 >> (2 * i)) & 3));
                }
            }
            (12, true) => {
                for x in 0..w {
                    let group = &row[x / 2 * 3..];
                    out.push(if x % 2 == 0 { ((group[0] as u16) << 4) | (group[2] as u16 & 0xf) } else { ((group[1] as u16) << 4) | (group[2] as u16 >> 4) });
                }
            }
            (_, false) => out.extend(row[..w * 2].chunks(2).map(|p| u16::from_le_bytes([p[0], p[1]]))),
            _ => bail!("unsupported raw layout"),
        }
    }
    Ok(out)
}

type M3 = [[f64; 3]; 3];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

fn inverse(m: &M3) -> Option<M3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let c = |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    Some([
        [c(1, 1, 2, 2) / det, -c(0, 1, 2, 2) / det, c(0, 1, 1, 2) / det],
        [-c(1, 0, 2, 2) / det, c(0, 0, 2, 2) / det, -c(0, 0, 1, 2) / det],
        [c(1, 0, 2, 1) / det, -c(0, 0, 2, 1) / det, c(0, 0, 1, 1) / det],
    ])
}

struct Ifd {
    entries: Vec<(u16, u16, u32, Vec<u8>)>,
}

impl Ifd {
    fn new() -> Self {
        Self { entries: Vec::new() }
    }
    fn add(&mut self, tag: u16, ty: u16, count: usize, bytes: Vec<u8>) {
        self.entries.push((tag, ty, count as u32, bytes));
    }
    fn short(&mut self, tag: u16, v: &[u16]) {
        self.add(tag, 3, v.len(), v.iter().flat_map(|x| x.to_le_bytes()).collect());
    }
    fn long(&mut self, tag: u16, v: &[u32]) {
        self.add(tag, 4, v.len(), v.iter().flat_map(|x| x.to_le_bytes()).collect());
    }
    fn byte(&mut self, tag: u16, v: &[u8]) {
        self.add(tag, 1, v.len(), v.to_vec());
    }
    fn ascii(&mut self, tag: u16, s: &str) {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        self.add(tag, 2, b.len(), b);
    }
    fn rational(&mut self, tag: u16, v: &[f64]) {
        let b = v.iter().flat_map(|x| {
            let d = 1_000_000u32;
            [((x * d as f64).round().max(0.0) as u32).to_le_bytes(), d.to_le_bytes()].concat()
        });
        self.add(tag, 5, v.len(), b.collect());
    }
    fn srational(&mut self, tag: u16, v: &[f64]) {
        let b = v.iter().flat_map(|x| {
            let d = 1_000_000i32;
            [((x * d as f64).round() as i32).to_le_bytes(), d.to_le_bytes()].concat()
        });
        self.add(tag, 10, v.len(), b.collect());
    }

    /// Serialise at `offset`; values that do not fit an entry follow the IFD.
    fn bytes(&mut self, offset: u32, next: u32) -> Vec<u8> {
        self.entries.sort_by_key(|e| e.0);
        let n = self.entries.len();
        let mut extra_at = offset + 2 + 12 * n as u32 + 4;
        let (mut head, mut extra) = (Vec::new(), Vec::new());
        head.extend((n as u16).to_le_bytes());
        for (tag, ty, count, value) in &self.entries {
            head.extend(tag.to_le_bytes());
            head.extend(ty.to_le_bytes());
            head.extend(count.to_le_bytes());
            if value.len() <= 4 {
                let mut v = value.clone();
                v.resize(4, 0);
                head.extend(v);
            } else {
                head.extend(extra_at.to_le_bytes());
                extra.extend(value);
                if value.len() % 2 == 1 {
                    extra.push(0);
                }
                extra_at = offset + 2 + 12 * n as u32 + 4 + extra.len() as u32;
            }
        }
        head.extend(next.to_le_bytes());
        head.extend(extra);
        head
    }

    fn size(&self) -> u32 {
        let extra: usize = self.entries.iter().filter(|e| e.3.len() > 4).map(|e| e.3.len() + e.3.len() % 2).sum();
        (2 + 12 * self.entries.len() + 4 + extra) as u32
    }
}

pub fn write(raw: &RawImage, still: &Still, path: &Path) -> Result<()> {
    let Some(l) = layout(&raw.format) else { bail!("not a Bayer format: {}", raw.format) };
    let pixels = unpack(raw, &l)?;
    let meta = &still.metadata;
    let white = (1u32 << l.bits) - 1;

    // SensorBlackLevels are 16-bit-scaled, in R, Gr, Gb, B order.
    let black16 = meta.values.get("SensorBlackLevels").cloned().unwrap_or_default();
    let black: Vec<u32> = (0..4)
        .map(|pos| {
            let channel = match (l.cfa[pos], pos / 2) {
                (0, _) => 0,
                (1, 0) => 1,
                (1, _) => 2,
                _ => 3,
            };
            black16.get(channel).map(|v| (*v as u32) >> (16 - l.bits)).unwrap_or(0)
        })
        .collect();

    let gains = meta.values.get("ColourGains").cloned().unwrap_or_else(|| vec![1.0, 1.0]);
    let (gr, gb) = (gains.first().copied().unwrap_or(1.0), gains.get(1).copied().unwrap_or(1.0));
    let ccm: M3 = match meta.values.get("ColourCorrectionMatrix") {
        Some(v) if v.len() == 9 => [[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]],
        _ => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };
    let rgb2xyz: M3 = [[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]];
    let wb: M3 = [[gr, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, gb]];
    let cam2xyz = mul(&mul(&rgb2xyz, &ccm), &wb);
    let mut xyz2cam = inverse(&cam2xyz).unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    // Normalise so the D65 white point maps to a maximum of 1.
    let d65 = [0.9505, 1.0, 1.0888];
    let peak = (0..3).map(|i| (0..3).map(|j| xyz2cam[i][j] * d65[j]).sum::<f64>()).fold(0.0f64, f64::max);
    if peak > 0.0 {
        xyz2cam.iter_mut().flatten().for_each(|x| *x /= peak);
    }

    let (w, h) = (raw.width, raw.height);
    let model = still.info.model.as_str();
    let datetime = relm4::gtk::glib::DateTime::now_local().ok().and_then(|d| d.format("%Y:%m:%d %H:%M:%S").ok()).map(|s| s.to_string()).unwrap_or_default();

    let mut exif = Ifd::new();
    if let Some(us) = meta.get("ExposureTime") {
        exif.rational(33434, &[us / 1e6]);
    }
    if let Some(g) = meta.get("AnalogueGain") {
        exif.short(34855, &[(g * meta.get("DigitalGain").unwrap_or(1.0) * 100.0).round().min(65535.0) as u16]);
    }

    let mut ifd = Ifd::new();
    ifd.long(254, &[0]);
    ifd.long(256, &[w]);
    ifd.long(257, &[h]);
    ifd.short(258, &[16]);
    ifd.short(259, &[1]);
    ifd.short(262, &[32803]);
    ifd.ascii(271, "Obscura");
    ifd.ascii(272, model);
    ifd.short(
        274,
        &[match still.info.rotation.rem_euclid(360) {
            90 => 6,
            180 => 3,
            270 => 8,
            _ => 1,
        }],
    );
    ifd.short(277, &[1]);
    ifd.long(278, &[h]);
    ifd.long(279, &[w * h * 2]);
    ifd.short(284, &[1]);
    ifd.ascii(305, "Obscura");
    ifd.ascii(306, &datetime);
    ifd.short(33421, &[2, 2]);
    ifd.byte(33422, &l.cfa);
    ifd.byte(50706, &[1, 4, 0, 0]);
    ifd.byte(50707, &[1, 1, 0, 0]);
    ifd.ascii(50708, model);
    ifd.short(50713, &[2, 2]);
    ifd.long(50714, &black);
    ifd.long(50717, &[white]);
    ifd.srational(50721, &xyz2cam.concat());
    ifd.rational(50728, &[1.0 / gr, 1.0, 1.0 / gb]);
    ifd.short(50778, &[21]);
    // Placeholders; both offsets depend on the final IFD size.
    ifd.long(273, &[0]);
    ifd.long(34665, &[0]);

    let ifd0_at = 8u32;
    let exif_at = ifd0_at + ifd.size();
    let strip_at = exif_at + exif.size();
    ifd.entries.retain(|e| e.0 != 273 && e.0 != 34665);
    ifd.long(273, &[strip_at]);
    ifd.long(34665, &[exif_at]);

    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"II\x2a\x00")?;
    f.write_all(&ifd0_at.to_le_bytes())?;
    f.write_all(&ifd.bytes(ifd0_at, 0))?;
    f.write_all(&exif.bytes(exif_at, 0))?;
    let mut strip = Vec::with_capacity(pixels.len() * 2);
    for p in pixels {
        strip.extend(p.to_le_bytes());
    }
    f.write_all(&strip)?;
    f.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpacks_csi2p() {
        let l = layout("SRGGB10_CSI2P").unwrap();
        assert_eq!((l.bits, l.packed, l.cfa), (10, true, [0, 1, 1, 2]));
        // pixels 1023, 0, 512, 3 -> high bytes ff 00 80 00, low bits 3,0,0,3
        let raw = RawImage { width: 4, height: 1, stride: 5, format: "SRGGB10_CSI2P".into(), data: vec![0xff, 0x00, 0x80, 0x00, 0b11_00_00_11] };
        assert_eq!(unpack(&raw, &l).unwrap(), vec![1023, 0, 512, 3]);
    }

    #[test]
    fn inverts() {
        let m = [[2.0, 0.0, 0.0], [0.0, 4.0, 0.0], [1.0, 0.0, 1.0]];
        let i = inverse(&m).unwrap();
        let id = mul(&m, &i);
        for r in 0..3 {
            for c in 0..3 {
                assert!((id[r][c] - (r == c) as u8 as f64).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn ifd_sizes_match_serialisation() {
        let mut ifd = Ifd::new();
        ifd.ascii(271, "Obscura");
        ifd.short(274, &[1]);
        ifd.srational(50721, &[1.0; 9]);
        let size = ifd.size();
        assert_eq!(ifd.bytes(8, 0).len() as u32, size);
    }
}
