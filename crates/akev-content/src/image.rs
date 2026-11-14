//! Image format sniffing and dimension extraction from header bytes.
//!
//! Formats: PNG (IHDR), JPEG (SOFn marker scan), GIF, WebP (VP8 / VP8L / VP8X), AVIF / HEIF
//! (ISO-BMFF `ispe` property; the largest one is reported since grids and thumbnails carry their
//! own), BMP, ICO / CUR (largest entry), TIFF (IFD0 width/length) and SVG (`width` / `height`
//! attributes in px or absolute units, else the `viewBox`). Unknown dimensions (e.g. a truncated
//! prefix) are `None` while the format is still reported.
//!
//! [`exif`] extracts the EXIF metadata summary (capture date, GPS, camera, lens, exposure
//! settings) that HTTP Toolkit shows in its image "EXIF banner".

use serde::Serialize;

mod exif;
pub use exif::{Exif, exif};

/// Detected image format and dimensions.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    /// png, jpeg, gif, webp, avif, heif, svg, ico, cur, bmp, tiff
    pub format: String,
    /// Width in pixels, when known.
    pub width: Option<u32>,
    /// Height in pixels, when known.
    pub height: Option<u32>,
}

fn info_of(format: &str, dims: Option<(u32, u32)>) -> ImageInfo {
    ImageInfo {
        format: format.to_string(),
        width: dims.map(|d| d.0),
        height: dims.map(|d| d.1),
    }
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn le24(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 3)?;
    Some(u32::from(s[0]) | u32::from(s[1]) << 8 | u32::from(s[2]) << 16)
}

/// Sniff image format & dimensions from the header bytes.
pub fn info(bytes: &[u8]) -> Option<ImageInfo> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let dims = (bytes.get(12..16) == Some(b"IHDR"))
            .then(|| Some((be32(bytes, 16)?, be32(bytes, 20)?)))
            .flatten();
        return Some(info_of("png", dims));
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(info_of("jpeg", jpeg_dims(bytes)));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        let dims = (|| Some((u32::from(le16(bytes, 6)?), u32::from(le16(bytes, 8)?))))();
        return Some(info_of("gif", dims));
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some(info_of("webp", webp_dims(bytes)));
    }
    if bytes.get(4..8) == Some(b"ftyp")
        && let Some(format) = bmff_image_brand(bytes)
    {
        return Some(info_of(format, bmff_ispe(bytes)));
    }
    if bytes.starts_with(b"BM")
        && let Some(dims) = bmp_dims(bytes)
    {
        return Some(info_of("bmp", Some(dims)));
    }
    if bytes.len() >= 6
        && bytes[0] == 0
        && bytes[1] == 0
        && (bytes[2] == 1 || bytes[2] == 2)
        && bytes[3] == 0
    {
        let count = le16(bytes, 4).unwrap_or(0);
        if count > 0 && count <= 512 {
            let format = if bytes[2] == 1 { "ico" } else { "cur" };
            return Some(info_of(format, ico_dims(bytes, count)));
        }
    }
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Some(info_of("tiff", tiff_dims(bytes)));
    }
    if is_svg(bytes) {
        return Some(info_of("svg", svg_dims(bytes)));
    }
    None
}

fn jpeg_dims(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 1 < b.len() {
        if b[i] != 0xFF {
            return None; // lost sync
        }
        // skip fill bytes
        while i < b.len() && b[i] == 0xFF {
            i += 1;
        }
        let marker = *b.get(i)?;
        i += 1;
        match marker {
            0xD8 | 0x01 | 0xD0..=0xD7 => continue, // no length
            0xD9 | 0xDA => return None,            // EOI / SOS before any SOF
            _ => {}
        }
        let len = usize::from(be16(b, i)?);
        if len < 2 {
            return None;
        }
        let is_sof = matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF);
        if is_sof {
            let h = be16(b, i + 3)?;
            let w = be16(b, i + 5)?;
            return Some((u32::from(w), u32::from(h)));
        }
        i += len;
    }
    None
}

fn webp_dims(b: &[u8]) -> Option<(u32, u32)> {
    match b.get(12..16)? {
        b"VP8 " => {
            // frame tag (3) + start code 9d 01 2a + 14-bit width/height
            if b.get(23..26)? != [0x9d, 0x01, 0x2a] {
                return None;
            }
            Some((
                u32::from(le16(b, 26)? & 0x3fff),
                u32::from(le16(b, 28)? & 0x3fff),
            ))
        }
        b"VP8L" => {
            if *b.get(20)? != 0x2f {
                return None;
            }
            let bits = le32(b, 21)?;
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8X" => Some((le24(b, 24)? + 1, le24(b, 27)? + 1)),
        _ => None,
    }
}

const AVIF_BRANDS: &[&[u8]] = &[b"avif", b"avis"];
const HEIF_BRANDS: &[&[u8]] = &[
    b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"hevm", b"hevs", b"mif1", b"msf1",
];

/// AVIF / HEIF brand from the `ftyp` box (major brand first, then compatible brands).
fn bmff_image_brand(b: &[u8]) -> Option<&'static str> {
    let size = be32(b, 0)? as usize;
    let end = size.min(b.len());
    if size < 16 {
        return None;
    }
    let mut brands = vec![b.get(8..12)?];
    let mut i = 16;
    while i + 4 <= end {
        brands.push(&b[i..i + 4]);
        i += 4;
    }
    if brands.iter().any(|br| AVIF_BRANDS.contains(br)) {
        return Some("avif");
    }
    if brands.iter().any(|br| HEIF_BRANDS.contains(br)) {
        return Some("heif");
    }
    None
}

/// Walk `meta` → `iprp` → `ipco` → `ispe` boxes; return the largest image spatial extent.
///
/// Containers are scanned with an explicit stack of `(start, end)` ranges. A malformed or
/// truncated box header ends the scan of its own container only, so dimensions inside an
/// already-found `meta` box survive a cut-off `mdat` that follows it.
fn bmff_ispe(b: &[u8]) -> Option<(u32, u32)> {
    let mut best: Option<(u32, u32)> = None;
    let mut stack: Vec<(usize, usize)> = vec![(0, b.len())];
    let mut visited = 0usize;
    while let Some((mut pos, end)) = stack.pop() {
        while pos + 8 <= end {
            visited += 1;
            if visited > 10_000 {
                return best;
            }
            let Some(size32) = be32(b, pos) else {
                break;
            };
            let kind = &b[pos + 4..pos + 8];
            let (header, size) = match size32 {
                0 => (8, end - pos),
                1 => match (be32(b, pos + 8), be32(b, pos + 12)) {
                    // 64-bit size; boxes beyond 4 GiB can't matter for a header scan.
                    (Some(0), Some(lo)) => (16, lo as usize),
                    _ => break,
                },
                n => (8, n as usize),
            };
            if size < header {
                break;
            }
            let box_end = pos.saturating_add(size).min(end);
            let body = pos + header;
            match kind {
                b"meta" => stack.push((body + 4, box_end)), // FullBox: skip version/flags
                b"iprp" | b"ipco" => stack.push((body, box_end)),
                b"ispe" => {
                    if let (Some(w), Some(h)) = (be32(b, body + 4), be32(b, body + 8)) {
                        let area = u64::from(w) * u64::from(h);
                        if best.is_none_or(|(bw, bh)| area > u64::from(bw) * u64::from(bh)) {
                            best = Some((w, h));
                        }
                    }
                }
                _ => {}
            }
            pos = pos.saturating_add(size);
        }
    }
    best
}

fn bmp_dims(b: &[u8]) -> Option<(u32, u32)> {
    let dib = le32(b, 14)?;
    match dib {
        12 => Some((u32::from(le16(b, 18)?), u32::from(le16(b, 20)?))),
        16 | 40 | 52 | 56 | 64 | 108 | 124 => {
            let w = le32(b, 18)? as i32;
            let h = le32(b, 22)? as i32;
            Some((w.unsigned_abs(), h.unsigned_abs()))
        }
        _ => None,
    }
}

fn ico_dims(b: &[u8], count: u16) -> Option<(u32, u32)> {
    let mut best: Option<(u32, u32)> = None;
    for i in 0..usize::from(count) {
        let at = 6 + i * 16;
        let Some(entry) = b.get(at..at + 16) else {
            break;
        };
        let w = if entry[0] == 0 {
            256
        } else {
            u32::from(entry[0])
        };
        let h = if entry[1] == 0 {
            256
        } else {
            u32::from(entry[1])
        };
        if best.is_none_or(|(bw, bh)| w * h > bw * bh) {
            best = Some((w, h));
        }
    }
    best
}

fn tiff_dims(b: &[u8]) -> Option<(u32, u32)> {
    let le = b[0] == b'I';
    let r16 = |at| if le { le16(b, at) } else { be16(b, at) };
    let r32 = |at| if le { le32(b, at) } else { be32(b, at) };
    let ifd = r32(4)? as usize;
    let n = usize::from(r16(ifd)?);
    let (mut w, mut h) = (None, None);
    for i in 0..n.min(512) {
        let e = ifd + 2 + i * 12;
        let tag = r16(e)?;
        let ty = r16(e + 2)?;
        let value = match ty {
            3 => u32::from(r16(e + 8)?),
            4 => r32(e + 8)?,
            _ => continue,
        };
        match tag {
            256 => w = Some(value),
            257 => h = Some(value),
            _ => {}
        }
        if let (Some(w), Some(h)) = (w, h) {
            return Some((w, h));
        }
    }
    None
}

/// Locate the root `<svg` start tag (skipping BOM, whitespace, XML declaration, comments,
/// doctype and processing instructions). Returns the byte range of the tag's attributes.
fn svg_tag(bytes: &[u8]) -> Option<&[u8]> {
    let b = &bytes[..bytes.len().min(64 * 1024)];
    let mut i = 0;
    let b = crate::util::strip_bom(b);
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let rest = b.get(i..)?;
        if rest.starts_with(b"<!--") {
            i += 4 + memchr::memmem::find(&rest[4..], b"-->")? + 3;
        } else if rest.starts_with(b"<?") || rest.starts_with(b"<!") {
            // xml decl / PI / doctype (doctype internal subsets with '>' inside are rare in SVG)
            let mut depth = 0i32;
            let mut j = 2;
            loop {
                match *rest.get(j)? {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    b'>' if depth <= 0 => break,
                    _ => {}
                }
                j += 1;
            }
            i += j + 1;
        } else if rest.len() >= 5
            && rest[..4].eq_ignore_ascii_case(b"<svg")
            && (rest[4].is_ascii_whitespace() || rest[4] == b'>' || rest[4] == b'/')
        {
            let mut j = 4;
            let mut quote = None;
            while j < rest.len() {
                let c = rest[j];
                match quote {
                    Some(q) if c == q => quote = None,
                    Some(_) => {}
                    None if c == b'"' || c == b'\'' => quote = Some(c),
                    None if c == b'>' => return Some(&rest[4..j]),
                    None => {}
                }
                j += 1;
            }
            return Some(&rest[4..]); // truncated tag: still use what we have
        } else {
            return None;
        }
    }
}

/// Whether the bytes are an SVG document (root element `<svg`).
pub fn is_svg(bytes: &[u8]) -> bool {
    svg_tag(bytes).is_some()
}

fn svg_attr<'a>(tag: &'a [u8], name: &[u8]) -> Option<&'a str> {
    let mut i = 0;
    while i < tag.len() {
        // find attribute name start
        while i < tag.len() && (tag[i].is_ascii_whitespace() || tag[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < tag.len() && !tag[i].is_ascii_whitespace() && tag[i] != b'=' {
            i += 1;
        }
        let attr = &tag[start..i];
        while i < tag.len() && tag[i].is_ascii_whitespace() {
            i += 1;
        }
        if tag.get(i) != Some(&b'=') {
            if i == start {
                i += 1;
            }
            continue;
        }
        i += 1;
        while i < tag.len() && tag[i].is_ascii_whitespace() {
            i += 1;
        }
        let (vstart, vend) = match tag.get(i) {
            Some(&q @ (b'"' | b'\'')) => {
                let s = i + 1;
                let e = memchr::memchr(q, &tag[s..]).map_or(tag.len(), |e| s + e);
                i = e + 1;
                (s, e)
            }
            _ => {
                let s = i;
                while i < tag.len() && !tag[i].is_ascii_whitespace() {
                    i += 1;
                }
                (s, i)
            }
        };
        if attr == name {
            return std::str::from_utf8(&tag[vstart..vend.min(tag.len())]).ok();
        }
    }
    None
}

/// Parse an SVG length into CSS pixels; relative units (`%`, `em`, ...) give `None`.
fn svg_length(v: &str) -> Option<f64> {
    let v = v.trim();
    let split = v
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')))
        .unwrap_or(v.len());
    let (num, unit) = v.split_at(split);
    let n: f64 = num.parse().ok()?;
    let px = match unit.trim() {
        "" | "px" => n,
        "pt" => n * 4.0 / 3.0,
        "pc" => n * 16.0,
        "in" => n * 96.0,
        "cm" => n * 96.0 / 2.54,
        "mm" => n * 96.0 / 25.4,
        "q" | "Q" => n * 96.0 / 101.6,
        _ => return None,
    };
    (px.is_finite() && px >= 0.0).then_some(px)
}

fn svg_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    let tag = svg_tag(bytes)?;
    let w = svg_attr(tag, b"width").and_then(svg_length);
    let h = svg_attr(tag, b"height").and_then(svg_length);
    let vb = svg_attr(tag, b"viewBox").and_then(|v| {
        let nums: Vec<f64> = v
            .split(|c: char| c.is_ascii_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();
        (nums.len() == 4 && nums[2] > 0.0 && nums[3] > 0.0).then(|| (nums[2], nums[3]))
    });
    let (w, h) = match (w, h, vb) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some((vw, vh))) => (w, w * vh / vw),
        (None, Some(h), Some((vw, vh))) => (h * vw / vh, h),
        (None, None, Some((vw, vh))) => (vw, vh),
        _ => return None,
    };
    let to_u32 = |x: f64| (x.is_finite() && x <= f64::from(u32::MAX)).then(|| x.round() as u32);
    Some((to_u32(w)?, to_u32(h)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dims(b: &[u8]) -> (String, Option<u32>, Option<u32>) {
        let i = info(b).expect("recognized");
        (i.format, i.width, i.height)
    }

    #[test]
    fn png() {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend_from_slice(&640u32.to_be_bytes());
        b.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(dims(&b), ("png".into(), Some(640), Some(480)));
        assert_eq!(dims(&b[..12]), ("png".into(), None, None));
    }

    #[test]
    fn jpeg() {
        // SOI, APP0 (len 16), DQT-ish (len 4), SOF0 (h=300, w=400)
        let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        b.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        b.extend_from_slice(&[0xFF, 0xFF, 0xDB, 0x00, 0x04, 0x00, 0x00]); // fill byte + DQT
        b.extend_from_slice(&[0xFF, 0xC2, 0x00, 0x11, 0x08, 0x01, 0x2C, 0x01, 0x90, 0x03]);
        assert_eq!(dims(&b), ("jpeg".into(), Some(400), Some(300)));
        // SOS before SOF → unknown dims
        let sos = [0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02];
        assert_eq!(dims(&sos), ("jpeg".into(), None, None));
        // DHT (C4) is not a SOF
        let dht = [
            0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00,
            0x10, 0x00, 0x20,
        ];
        assert_eq!(dims(&dht), ("jpeg".into(), Some(32), Some(16)));
    }

    #[test]
    fn gif_bmp_ico_tiff() {
        assert_eq!(
            dims(b"GIF89a\x10\x00\x20\x00"),
            ("gif".into(), Some(16), Some(32))
        );
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0u8; 12]);
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&100i32.to_le_bytes());
        bmp.extend_from_slice(&(-50i32).to_le_bytes());
        assert_eq!(dims(&bmp), ("bmp".into(), Some(100), Some(50)));
        let mut core = b"BM".to_vec();
        core.extend_from_slice(&[0u8; 12]);
        core.extend_from_slice(&12u32.to_le_bytes());
        core.extend_from_slice(&7u16.to_le_bytes());
        core.extend_from_slice(&9u16.to_le_bytes());
        assert_eq!(dims(&core), ("bmp".into(), Some(7), Some(9)));
        assert!(info(b"BMx").is_none());
        let mut ico = vec![0, 0, 1, 0, 2, 0];
        ico.extend_from_slice(&[16, 16, 0, 0, 1, 0, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        ico.extend_from_slice(&[0, 0, 0, 0, 1, 0, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(dims(&ico), ("ico".into(), Some(256), Some(256)));
        let mut tiff = b"II*\0".to_vec();
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        tiff.extend_from_slice(&[0x00, 0x01, 3, 0, 1, 0, 0, 0, 0x20, 0x03, 0, 0]); // width 800 (short)
        tiff.extend_from_slice(&[0x01, 0x01, 4, 0, 1, 0, 0, 0, 0x58, 0x02, 0, 0]); // length 600 (long)
        assert_eq!(dims(&tiff), ("tiff".into(), Some(800), Some(600)));
    }

    #[test]
    fn webp_variants() {
        let riff = |chunk: &[u8], data: &[u8]| {
            let mut b = b"RIFF\0\0\0\0WEBP".to_vec();
            b.extend_from_slice(chunk);
            b.extend_from_slice(&(data.len() as u32).to_le_bytes());
            b.extend_from_slice(data);
            b
        };
        let mut vp8 = vec![0x30, 0x01, 0x00, 0x9d, 0x01, 0x2a];
        vp8.extend_from_slice(&320u16.to_le_bytes());
        vp8.extend_from_slice(&(240u16 | 0x4000).to_le_bytes()); // scale bits masked off
        assert_eq!(
            dims(&riff(b"VP8 ", &vp8)),
            ("webp".into(), Some(320), Some(240))
        );
        let bits: u32 = (100 - 1) | ((200 - 1) << 14);
        let mut vp8l = vec![0x2f];
        vp8l.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(
            dims(&riff(b"VP8L", &vp8l)),
            ("webp".into(), Some(100), Some(200))
        );
        let mut vp8x = vec![0x10, 0, 0, 0];
        vp8x.extend_from_slice(&[0xff, 0x03, 0x00]); // 1024
        vp8x.extend_from_slice(&[0xff, 0x02, 0x00]); // 768
        assert_eq!(
            dims(&riff(b"VP8X", &vp8x)),
            ("webp".into(), Some(1024), Some(768))
        );
    }

    fn bmff_box(kind: &[u8], body: &[u8]) -> Vec<u8> {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b.extend_from_slice(body);
        b
    }

    #[test]
    fn avif_and_heif() {
        let ispe = |w: u32, h: u32| {
            let mut body = vec![0, 0, 0, 0];
            body.extend_from_slice(&w.to_be_bytes());
            body.extend_from_slice(&h.to_be_bytes());
            bmff_box(b"ispe", &body)
        };
        let mut ipco_body = ispe(64, 64);
        ipco_body.extend(ispe(1920, 1080));
        let iprp = bmff_box(b"iprp", &bmff_box(b"ipco", &ipco_body));
        let mut meta_body = vec![0, 0, 0, 0];
        meta_body.extend(bmff_box(b"hdlr", &[0u8; 20]));
        meta_body.extend(iprp);
        let meta = bmff_box(b"meta", &meta_body);
        let mut avif = bmff_box(b"ftyp", b"avif\0\0\0\0mif1miaf");
        avif.extend(meta.clone());
        assert_eq!(dims(&avif), ("avif".into(), Some(1920), Some(1080)));
        let mut heic = bmff_box(b"ftyp", b"heic\0\0\0\0mif1heic");
        heic.extend(meta);
        assert_eq!(dims(&heic), ("heif".into(), Some(1920), Some(1080)));
        // An mp4 is not an image.
        assert!(info(&bmff_box(b"ftyp", b"isom\0\0\0\0isomavc1")).is_none());
        // A truncated large-size box header after the ispe keeps the dimensions found so far.
        let mut cut = bmff_box(b"ftyp", b"avif\0\0\0\0mif1miaf");
        cut.extend(bmff_box(
            b"meta",
            &[
                &[0u8, 0, 0, 0][..],
                &bmff_box(b"iprp", &bmff_box(b"ipco", &ispe(8, 4))),
            ]
            .concat(),
        ));
        cut.extend_from_slice(&[0, 0, 0, 1, b'm', b'd', b'a', b't', 0, 0]);
        assert_eq!(dims(&cut), ("avif".into(), Some(8), Some(4)));
    }

    #[test]
    fn svg() {
        let s = br#"<?xml version="1.0"?><!-- c --><!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "x"><svg xmlns="http://www.w3.org/2000/svg" width="120px" height='60'>"#;
        assert_eq!(dims(s), ("svg".into(), Some(120), Some(60)));
        assert_eq!(
            dims(b"<svg viewBox=\"0 0 24 12\"/>"),
            ("svg".into(), Some(24), Some(12))
        );
        assert_eq!(
            dims(b"<svg width=\"48\" viewBox=\"0,0,24,12\">"),
            ("svg".into(), Some(48), Some(24))
        );
        assert_eq!(
            dims(b"<svg width=\"100%\" height=\"100%\">"),
            ("svg".into(), None, None)
        );
        assert_eq!(
            dims(b"\xEF\xBB\xBF  <SVG width=\"1in\" height=\"72pt\">"),
            ("svg".into(), Some(96), Some(96))
        );
        assert_eq!(
            dims(b"<svg data-x=\"width=5\" width=\"7\" height=\"8\">"),
            ("svg".into(), Some(7), Some(8))
        );
        assert!(info(b"<html><svg></svg></html>").is_none());
        assert!(info(b"<svgfoo>").is_none());
        assert!(is_svg(b"<svg>"));
    }

    #[test]
    fn unknown() {
        assert!(info(b"").is_none());
        assert!(info(b"hello world").is_none());
        assert!(info(&[0, 0, 1, 0, 0, 0]).is_none()); // ico with 0 entries
    }
}
