//! EXIF metadata extraction (the subset HTTP Toolkit's image "EXIF banner" shows).
//!
//! Sources: JPEG `APP1` `Exif\0\0` segments, PNG `eXIf` chunks, WebP `EXIF` chunks and bare TIFF
//! files. Only IFD0 and its Exif / GPS sub-IFDs are read (no IFD chains, so no cycles); every
//! read is bounds-checked and entry counts are capped, so hostile input costs O(1) work beyond
//! locating the segment.

use serde::Serialize;

/// EXIF metadata summary.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exif {
    /// Capture date: `DateTimeOriginal`, else `DateTimeDigitized`, else `DateTime`, as ISO 8601
    /// (`2024-05-01T12:30:00`). EXIF times carry no zone; the matching `OffsetTime*` tag is
    /// appended (`2024-05-01T12:30:00+02:00`) when present.
    pub date: Option<String>,
    /// Camera manufacturer (`Make`).
    pub make: Option<String>,
    /// Camera model (`Model`).
    pub model: Option<String>,
    /// Lens model (`LensModel`, else `LensMake`).
    pub lens: Option<String>,
    /// Software that produced the image.
    pub software: Option<String>,
    /// Orientation (1–8).
    pub orientation: Option<u16>,
    /// Focal length in millimetres.
    pub focal_length_mm: Option<f64>,
    /// Exposure time in seconds.
    pub exposure_time: Option<f64>,
    /// Aperture (f-number).
    pub f_number: Option<f64>,
    /// ISO speed.
    pub iso: Option<u32>,
    /// GPS latitude in decimal degrees (negative = south).
    pub latitude: Option<f64>,
    /// GPS longitude in decimal degrees (negative = west).
    pub longitude: Option<f64>,
    /// GPS altitude in metres (negative = below sea level).
    pub altitude: Option<f64>,
    /// Number of tags present in IFD0 and the Exif / GPS IFDs.
    pub tag_count: usize,
    /// One-line summary like HTTP Toolkit's collapsed banner: up to eight parts joined with
    /// ` · ` (date, GPS, camera, lens, focal length, exposure, aperture, ISO), or
    /// `N properties` when none of those are present.
    pub summary: String,
}

/// Extract EXIF metadata from a JPEG, PNG, WebP or TIFF image. `None` when the image carries no
/// (parseable) EXIF block.
pub fn exif(bytes: &[u8]) -> Option<Exif> {
    let tiff = locate(bytes)?;
    parse_tiff(tiff)
}

/// Find the TIFF-structured EXIF payload inside a container.
fn locate(b: &[u8]) -> Option<&[u8]> {
    if b.starts_with(&[0xFF, 0xD8]) {
        return jpeg_app1(b);
    }
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png_exif(b);
    }
    if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        return webp_exif(b);
    }
    if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        return Some(b);
    }
    None
}

fn strip_exif_header(d: &[u8]) -> &[u8] {
    d.strip_prefix(b"Exif\0\0").unwrap_or(d)
}

fn jpeg_app1(b: &[u8]) -> Option<&[u8]> {
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        match marker {
            0xFF => {
                i += 1; // fill byte
                continue;
            }
            0xD8 | 0x01 | 0xD0..=0xD7 => {
                i += 2;
                continue;
            }
            0xD9 | 0xDA => return None, // EOI / start of scan: metadata comes before
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([b[i + 2], b[i + 3]]));
        if len < 2 {
            return None;
        }
        let data = b.get(i + 4..(i + 2 + len).min(b.len()))?;
        if marker == 0xE1 && data.starts_with(b"Exif\0\0") {
            return Some(&data[6..]);
        }
        i += 2 + len;
    }
    None
}

fn png_exif(b: &[u8]) -> Option<&[u8]> {
    let mut i = 8;
    while i + 8 <= b.len() {
        let len = u32::from_be_bytes(b[i..i + 4].try_into().ok()?) as usize;
        let kind = &b[i + 4..i + 8];
        let data_start = i + 8;
        let data_end = data_start.checked_add(len)?;
        if kind == b"eXIf" {
            return Some(strip_exif_header(b.get(data_start..data_end.min(b.len()))?));
        }
        if kind == b"IEND" {
            return None;
        }
        i = data_end.checked_add(4)?; // CRC
    }
    None
}

fn webp_exif(b: &[u8]) -> Option<&[u8]> {
    let mut i = 12;
    while i + 8 <= b.len() {
        let kind = &b[i..i + 4];
        let len = u32::from_le_bytes(b[i + 4..i + 8].try_into().ok()?) as usize;
        let data_start = i + 8;
        let data_end = data_start.checked_add(len)?;
        if kind == b"EXIF" {
            return Some(strip_exif_header(b.get(data_start..data_end.min(b.len()))?));
        }
        i = data_end.checked_add(len & 1)?; // chunks are padded to even sizes
    }
    None
}

/// Maximum IFD entries read per directory.
const MAX_ENTRIES: usize = 1024;

/// Size in bytes of one value of a TIFF field type (`None` for unknown types).
fn unit_size(ty: u16) -> Option<usize> {
    match ty {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 => Some(4),
        5 | 10 | 12 => Some(8),
        _ => None,
    }
}

struct Tiff<'a> {
    b: &'a [u8],
    le: bool,
}

#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    ty: u16,
    count: u32,
    /// Offset of the 4-byte value/offset field.
    field: usize,
}

impl<'a> Tiff<'a> {
    fn u16(&self, at: usize) -> Option<u16> {
        let s: [u8; 2] = self.b.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(s)
        } else {
            u16::from_be_bytes(s)
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let s: [u8; 4] = self.b.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if self.le {
            u32::from_le_bytes(s)
        } else {
            u32::from_be_bytes(s)
        })
    }

    fn entries(&self, ifd: usize) -> Vec<Entry> {
        let Some(n) = self.u16(ifd) else {
            return Vec::new();
        };
        (0..usize::from(n).min(MAX_ENTRIES))
            .map_while(|k| {
                let e = ifd + 2 + k * 12;
                Some(Entry {
                    tag: self.u16(e)?,
                    ty: self.u16(e + 2)?,
                    count: self.u32(e + 4)?,
                    field: e + 8,
                })
            })
            .collect()
    }

    /// Byte range of an entry's value (inline when it fits in 4 bytes), checked to lie
    /// entirely within the buffer.
    fn value_range(&self, e: &Entry) -> Option<(usize, usize)> {
        let size = (e.count as usize).checked_mul(unit_size(e.ty)?)?;
        let start = if size <= 4 {
            e.field
        } else {
            self.u32(e.field)? as usize
        };
        let end = start.checked_add(size)?;
        (end <= self.b.len()).then_some((start, end))
    }

    /// The bytes of an entry's value.
    fn data(&self, e: &Entry) -> Option<&'a [u8]> {
        let (start, end) = self.value_range(e)?;
        self.b.get(start..end)
    }

    fn ascii(&self, e: &Entry) -> Option<String> {
        if e.ty != 2 && e.ty != 7 {
            return None;
        }
        let d = self.data(e)?;
        let d = &d[..d.iter().position(|&c| c == 0).unwrap_or(d.len())];
        let s = String::from_utf8_lossy(d).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    /// The `i`-th numeric value (integer or rational) as f64.
    fn number(&self, e: &Entry, i: usize) -> Option<f64> {
        if i >= e.count as usize {
            return None;
        }
        let (base, _) = self.value_range(e)?;
        match e.ty {
            1 => self.b.get(base + i).map(|&v| f64::from(v)),
            6 => self.b.get(base + i).map(|&v| f64::from(v as i8)),
            3 => self.u16(base + i * 2).map(f64::from),
            8 => self.u16(base + i * 2).map(|v| f64::from(v as i16)),
            4 => self.u32(base + i * 4).map(f64::from),
            9 => self.u32(base + i * 4).map(|v| f64::from(v as i32)),
            5 | 10 => {
                let n = self.u32(base + i * 8)?;
                let dn = self.u32(base + i * 8 + 4)?;
                if dn == 0 {
                    return None;
                }
                Some(if e.ty == 5 {
                    f64::from(n) / f64::from(dn)
                } else {
                    f64::from(n as i32) / f64::from(dn as i32)
                })
            }
            _ => None,
        }
        .filter(|v| v.is_finite())
    }
}

fn parse_tiff(b: &[u8]) -> Option<Exif> {
    let le = match b.get(..4)? {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let t = Tiff { b, le };
    let ifd0 = t.u32(4)? as usize;
    let ifd0_entries = t.entries(ifd0);
    if ifd0_entries.is_empty() {
        return None;
    }
    let pointer = |entries: &[Entry], tag: u16| {
        entries
            .iter()
            .find(|e| e.tag == tag)
            .and_then(|e| t.number(e, 0))
            .map(|v| v as usize)
            .filter(|&off| off != ifd0)
    };
    let exif_entries = pointer(&ifd0_entries, 0x8769).map_or_else(Vec::new, |o| t.entries(o));
    let gps_entries = pointer(&ifd0_entries, 0x8825).map_or_else(Vec::new, |o| t.entries(o));

    let find = |entries: &[Entry], tag: u16| entries.iter().find(|e| e.tag == tag).copied();
    let ascii0 = |tag| find(&ifd0_entries, tag).and_then(|e| t.ascii(&e));
    let ascii_x = |tag| find(&exif_entries, tag).and_then(|e| t.ascii(&e));
    let num_x = |tag| find(&exif_entries, tag).and_then(|e| t.number(&e, 0));

    let mut out = Exif {
        make: ascii0(0x010F),
        model: ascii0(0x0110),
        software: ascii0(0x0131),
        orientation: find(&ifd0_entries, 0x0112)
            .and_then(|e| t.number(&e, 0))
            .map(|v| v as u16),
        lens: ascii_x(0xA434).or_else(|| ascii_x(0xA433)),
        focal_length_mm: num_x(0x920A).filter(|v| *v > 0.0),
        exposure_time: num_x(0x829A).filter(|v| *v > 0.0),
        f_number: num_x(0x829D).filter(|v| *v > 0.0),
        iso: num_x(0x8827)
            .or_else(|| num_x(0x8833))
            .map(|v| v as u32)
            .filter(|v| *v > 0),
        tag_count: ifd0_entries.len() + exif_entries.len() + gps_entries.len(),
        ..Exif::default()
    };
    // (date tag, offset tag)
    let dates = [
        (ascii_x(0x9003), ascii_x(0x9011)),
        (ascii_x(0x9004), ascii_x(0x9012)),
        (ascii0(0x0132), ascii_x(0x9010)),
    ];
    out.date = dates
        .into_iter()
        .find_map(|(d, off)| iso_date(d.as_deref()?, off.as_deref()));

    if !gps_entries.is_empty() {
        let g = |tag| find(&gps_entries, tag);
        let coord = |value_tag: u16, ref_tag: u16, negative: u8| {
            let e = g(value_tag)?;
            let deg = t.number(&e, 0)?;
            let min = t.number(&e, 1).unwrap_or(0.0);
            let sec = t.number(&e, 2).unwrap_or(0.0);
            let v = deg + min / 60.0 + sec / 3600.0;
            let r = g(ref_tag).and_then(|e| t.ascii(&e));
            let neg = r.is_some_and(|r| r.as_bytes().first() == Some(&negative));
            Some(if neg { -v } else { v })
        };
        out.latitude = coord(0x0002, 0x0001, b'S').filter(|v| v.abs() <= 90.0);
        out.longitude = coord(0x0004, 0x0003, b'W').filter(|v| v.abs() <= 180.0);
        out.altitude = g(0x0006).and_then(|e| t.number(&e, 0)).map(|a| {
            let below = g(0x0005)
                .and_then(|e| t.number(&e, 0))
                .is_some_and(|r| r == 1.0);
            if below { -a } else { a }
        });
    }
    out.summary = summary(&out);
    Some(out)
}

/// `YYYY:MM:DD HH:MM:SS` → `YYYY-MM-DDTHH:MM:SS` (+ offset). Blank/zeroed dates → None.
fn iso_date(d: &str, offset: Option<&str>) -> Option<String> {
    let d = d.trim();
    let bytes = d.as_bytes();
    if bytes.len() < 19 || !bytes[..19].iter().any(|c| (b'1'..=b'9').contains(c)) {
        return None;
    }
    let ok = bytes[..19].iter().enumerate().all(|(i, c)| match i {
        4 | 7 => matches!(c, b':' | b'-'),
        10 => matches!(c, b' ' | b'T'),
        13 | 16 => *c == b':',
        _ => c.is_ascii_digit(),
    });
    if !ok {
        return None;
    }
    let mut s = format!("{}-{}-{}T{}", &d[0..4], &d[5..7], &d[8..10], &d[11..19]);
    if let Some(off) = offset.map(str::trim) {
        let ob = off.as_bytes();
        if ob.len() == 6
            && matches!(ob[0], b'+' | b'-')
            && ob[3] == b':'
            && [1, 2, 4, 5].iter().all(|&i| ob[i].is_ascii_digit())
        {
            s.push_str(off);
        }
    }
    Some(s)
}

/// Format with up to `decimals` places, trailing zeros trimmed.
fn trim_num(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn summary(e: &Exif) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = &e.date {
        parts.push(d.clone());
    }
    if let (Some(lat), Some(lon)) = (e.latitude, e.longitude) {
        parts.push(format!(
            "{:.4}°{} {:.4}°{}",
            lat.abs(),
            if lat < 0.0 { 'S' } else { 'N' },
            lon.abs(),
            if lon < 0.0 { 'W' } else { 'E' }
        ));
    }
    let camera = match (&e.make, &e.model) {
        (Some(make), Some(model))
            if model
                .to_ascii_lowercase()
                .starts_with(&make.to_ascii_lowercase()) =>
        {
            Some(model.clone())
        }
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (Some(x), None) | (None, Some(x)) => Some(x.clone()),
        (None, None) => None,
    };
    parts.extend(camera);
    parts.extend(e.lens.clone());
    if let Some(f) = e.focal_length_mm {
        parts.push(format!("{}mm", trim_num(f, 1)));
    }
    if let Some(x) = e.exposure_time {
        parts.push(if x < 1.0 {
            format!("1/{}s", (1.0 / x).round())
        } else {
            format!("{}s", trim_num(x, 1))
        });
    }
    if let Some(f) = e.f_number {
        parts.push(format!("f/{}", trim_num(f, 1)));
    }
    if let Some(iso) = e.iso {
        parts.push(format!("ISO {iso}"));
    }
    if parts.is_empty() {
        let n = e.tag_count;
        format!("{n} {}", if n == 1 { "property" } else { "properties" })
    } else {
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny big-/little-endian TIFF builder: IFD0 (+ Exif and GPS IFDs).
    struct TiffBuilder {
        le: bool,
    }

    /// (tag, type, count, raw value bytes in the file's byte order)
    type RawEntry = (u16, u16, u32, Vec<u8>);

    impl TiffBuilder {
        fn new(le: bool) -> Self {
            TiffBuilder { le }
        }
        fn u16b(&self, v: u16) -> [u8; 2] {
            if self.le {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        }
        fn u32b(&self, v: u32) -> [u8; 4] {
            if self.le {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        }
        fn ascii(&self, tag: u16, s: &str) -> RawEntry {
            let mut v = s.as_bytes().to_vec();
            v.push(0);
            (tag, 2, v.len() as u32, v)
        }
        fn short(&self, tag: u16, v: u16) -> RawEntry {
            (tag, 3, 1, self.u16b(v).to_vec())
        }
        fn rationals(&self, tag: u16, vals: &[(u32, u32)]) -> RawEntry {
            let mut v = Vec::new();
            for (n, d) in vals {
                v.extend_from_slice(&self.u32b(*n));
                v.extend_from_slice(&self.u32b(*d));
            }
            (tag, 5, vals.len() as u32, v)
        }

        /// Build a TIFF with IFD0 entries plus optional Exif / GPS IFDs (pointers added
        /// automatically).
        fn build(&self, ifd0: Vec<RawEntry>, exif: Vec<RawEntry>, gps: Vec<RawEntry>) -> Vec<u8> {
            let mut out = if self.le {
                b"II*\0".to_vec()
            } else {
                b"MM\0*".to_vec()
            };
            out.extend_from_slice(&self.u32b(8));
            let ifd_size = |n: usize| 2 + n * 12 + 4;
            let n0 = ifd0.len() + usize::from(!exif.is_empty()) + usize::from(!gps.is_empty());
            let exif_off = 8 + ifd_size(n0);
            let gps_off = exif_off
                + if exif.is_empty() {
                    0
                } else {
                    ifd_size(exif.len())
                };
            let mut data_off = gps_off
                + if gps.is_empty() {
                    0
                } else {
                    ifd_size(gps.len())
                };
            let mut ifd0 = ifd0;
            if !exif.is_empty() {
                ifd0.push((0x8769, 4, 1, self.u32b(exif_off as u32).to_vec()));
            }
            if !gps.is_empty() {
                ifd0.push((0x8825, 4, 1, self.u32b(gps_off as u32).to_vec()));
            }
            let mut data = Vec::new();
            for dir in [ifd0, exif, gps] {
                if dir.is_empty() {
                    continue;
                }
                out.extend_from_slice(&self.u16b(dir.len() as u16));
                for (tag, ty, count, raw) in dir {
                    out.extend_from_slice(&self.u16b(tag));
                    out.extend_from_slice(&self.u16b(ty));
                    out.extend_from_slice(&self.u32b(count));
                    if raw.len() <= 4 {
                        let mut f = raw.clone();
                        f.resize(4, 0);
                        out.extend_from_slice(&f);
                    } else {
                        out.extend_from_slice(&self.u32b(data_off as u32));
                        data_off += raw.len();
                        data.extend_from_slice(&raw);
                    }
                }
                out.extend_from_slice(&self.u32b(0));
            }
            out.extend_from_slice(&data);
            out
        }
    }

    fn sample_tiff(le: bool) -> Vec<u8> {
        let t = TiffBuilder::new(le);
        t.build(
            vec![
                t.ascii(0x010F, "Canon"),
                t.ascii(0x0110, "Canon EOS 5D Mark IV"),
                t.short(0x0112, 1),
                t.ascii(0x0132, "2020:01:01 00:00:00"),
            ],
            vec![
                t.rationals(0x829A, &[(1, 250)]),
                t.rationals(0x829D, &[(28, 10)]),
                t.short(0x8827, 400),
                t.ascii(0x9003, "2023:07:14 18:05:09"),
                t.ascii(0x9011, "+02:00"),
                t.rationals(0x920A, &[(50, 1)]),
                t.ascii(0xA434, "EF50mm f/1.8 STM"),
            ],
            vec![
                t.ascii(0x0001, "N"),
                t.rationals(0x0002, &[(48, 1), (51, 1), (2412, 100)]),
                t.ascii(0x0003, "W"),
                t.rationals(0x0004, &[(2, 1), (17, 1), (4008, 100)]),
                (0x0005, 1, 1, vec![0]),
                t.rationals(0x0006, &[(355, 10)]),
            ],
        )
    }

    #[test]
    fn full_exif_both_byte_orders() {
        for le in [true, false] {
            let e = exif(&sample_tiff(le)).unwrap();
            assert_eq!(e.make.as_deref(), Some("Canon"));
            assert_eq!(e.model.as_deref(), Some("Canon EOS 5D Mark IV"));
            assert_eq!(e.orientation, Some(1));
            assert_eq!(e.date.as_deref(), Some("2023-07-14T18:05:09+02:00"));
            assert_eq!(e.exposure_time, Some(0.004));
            assert_eq!(e.f_number, Some(2.8));
            assert_eq!(e.iso, Some(400));
            assert_eq!(e.focal_length_mm, Some(50.0));
            assert_eq!(e.lens.as_deref(), Some("EF50mm f/1.8 STM"));
            let lat = e.latitude.unwrap();
            let lon = e.longitude.unwrap();
            assert!((lat - 48.8567).abs() < 1e-4, "{lat}");
            assert!((lon + 2.2945).abs() < 1e-4, "{lon}");
            assert_eq!(e.altitude, Some(35.5));
            assert_eq!(e.tag_count, 6 + 7 + 6);
            assert_eq!(
                e.summary,
                "2023-07-14T18:05:09+02:00 · 48.8567°N 2.2945°W · Canon EOS 5D Mark IV · \
                 EF50mm f/1.8 STM · 50mm · 1/250s · f/2.8 · ISO 400"
            );
        }
    }

    #[test]
    fn containers() {
        let tiff = sample_tiff(false);
        // JPEG: SOI, APP0, APP1 Exif, SOS
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0, 0];
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        jpeg.extend_from_slice(&[0xFF, 0xE1]);
        jpeg.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(&app1);
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        assert_eq!(exif(&jpeg).unwrap().iso, Some(400));
        // PNG eXIf chunk
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0u8; 13 + 4]);
        png.extend_from_slice(&(tiff.len() as u32).to_be_bytes());
        png.extend_from_slice(b"eXIf");
        png.extend_from_slice(&tiff);
        png.extend_from_slice(&[0u8; 4]);
        assert_eq!(exif(&png).unwrap().f_number, Some(2.8));
        // WebP EXIF chunk (odd size padded), with the optional "Exif\0\0" prefix
        let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
        webp.extend_from_slice(b"VP8X");
        webp.extend_from_slice(&10u32.to_le_bytes());
        webp.extend_from_slice(&[0u8; 10]);
        webp.extend_from_slice(b"EXIF");
        webp.extend_from_slice(&(app1.len() as u32).to_le_bytes());
        webp.extend_from_slice(&app1);
        assert_eq!(exif(&webp).unwrap().focal_length_mm, Some(50.0));
        // No EXIF anywhere
        assert_eq!(exif(&jpeg[..8]), None);
        assert_eq!(exif(b"\x89PNG\r\n\x1a\n"), None);
        assert_eq!(exif(b"GIF89a"), None);
    }

    #[test]
    fn minimal_and_hostile() {
        let t = TiffBuilder::new(true);
        // Only a software tag: summary falls back to the property count.
        let e = exif(&t.build(vec![t.ascii(0x0131, "GIMP")], vec![], vec![])).unwrap();
        assert_eq!(e.software.as_deref(), Some("GIMP"));
        assert_eq!(e.summary, "1 property");
        // Make + model that doesn't repeat the make.
        let e = exif(&t.build(
            vec![t.ascii(0x010F, "Apple"), t.ascii(0x0110, "iPhone 15")],
            vec![t.rationals(0x829A, &[(2, 1)])],
            vec![],
        ))
        .unwrap();
        assert_eq!(e.summary, "Apple iPhone 15 · 2s");
        // Zeroed dates and zero denominators are ignored.
        let e = exif(&t.build(
            vec![t.ascii(0x0132, "0000:00:00 00:00:00")],
            vec![t.rationals(0x829D, &[(5, 0)])],
            vec![],
        ))
        .unwrap();
        assert_eq!((e.date, e.f_number), (None, None));
        // Truncations at every length never panic; offsets pointing anywhere are fine.
        let full = sample_tiff(true);
        for n in 0..full.len() {
            let _ = exif(&full[..n]);
        }
        let mut bad = full.clone();
        bad[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(exif(&bad), None);
        // An Exif pointer back to IFD0 is ignored (no cycles).
        let looped = t.build(
            vec![
                t.ascii(0x010F, "X"),
                (0x8769, 4, 1, 8u32.to_le_bytes().to_vec()),
            ],
            vec![],
            vec![],
        );
        assert_eq!(exif(&looped).unwrap().make.as_deref(), Some("X"));
    }
}
