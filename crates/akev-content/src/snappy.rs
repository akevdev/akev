//! Snappy decompression (for `grpc-encoding: snappy`).
//!
//! Both formats are accepted: the raw block format, and the framing ("stream") format that
//! starts with the `sNaPpY` stream identifier (what Go's snappy gRPC codecs emit). Frame
//! checksums are not verified (this is a viewer). Output is capped, and every copy is
//! bounds-checked, so corrupt or hostile input can only produce an error.

/// Framing-format stream identifier chunk.
const STREAM_ID: &[u8] = b"\xff\x06\x00\x00sNaPpY";

use crate::decode::DecodeFailure;

/// Decompress a raw snappy block or a framed snappy stream, failing beyond `max` bytes
/// ([`DecodeFailure::TooLarge`]).
pub(crate) fn decompress(input: &[u8], max: usize) -> Result<Vec<u8>, DecodeFailure> {
    if input.starts_with(STREAM_ID) {
        decompress_framed(input, max)
    } else {
        let mut out = Vec::new();
        decompress_block(input, &mut out, max)?;
        Ok(out)
    }
}

fn decompress_framed(input: &[u8], max: usize) -> Result<Vec<u8>, DecodeFailure> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < input.len() {
        let header = input.get(i..i + 4).ok_or("truncated snappy frame header")?;
        let kind = header[0];
        let len =
            usize::from(header[1]) | usize::from(header[2]) << 8 | usize::from(header[3]) << 16;
        let data = input
            .get(i + 4..i + 4 + len)
            .ok_or("truncated snappy frame")?;
        match kind {
            // compressed / uncompressed data: 4-byte masked CRC-32C, then the payload
            0x00 => decompress_block(data.get(4..).ok_or("short snappy frame")?, &mut out, max)?,
            0x01 => {
                let payload = data.get(4..).ok_or("short snappy frame")?;
                if out.len() + payload.len() > max {
                    return Err(DecodeFailure::TooLarge(max));
                }
                out.extend_from_slice(payload);
            }
            0xff if data == &STREAM_ID[4..] => {}
            0x80..=0xfe => {} // padding / skippable
            _ => {
                return Err(DecodeFailure::Io(format!(
                    "unsupported snappy frame type 0x{kind:02x}"
                )));
            }
        }
        i += 4 + len;
    }
    Ok(out)
}

/// Decompress one raw snappy block, appending to `out`.
fn decompress_block(b: &[u8], out: &mut Vec<u8>, max: usize) -> Result<(), DecodeFailure> {
    let mut i = 0;
    let declared = crate::protobuf::read_varint(b, &mut i).ok_or("invalid snappy length")?;
    let declared = usize::try_from(declared).map_err(|_| "invalid snappy length")?;
    let base = out.len();
    if base.saturating_add(declared) > max {
        return Err(DecodeFailure::TooLarge(max));
    }
    // Never trust the declared size for allocation beyond what the input could produce.
    out.reserve(declared.min(b.len().saturating_mul(64)));
    let target = base + declared;
    while i < b.len() {
        let tag = b[i];
        i += 1;
        let (len, offset) = match tag & 3 {
            0 => {
                let mut len = usize::from(tag >> 2);
                if len >= 60 {
                    let n = len - 59;
                    let bytes = b.get(i..i + n).ok_or("truncated snappy literal")?;
                    len = bytes
                        .iter()
                        .rev()
                        .fold(0usize, |acc, &x| acc << 8 | usize::from(x));
                    i += n;
                }
                let len = len + 1;
                let lit = b.get(i..i + len).ok_or("truncated snappy literal")?;
                if out.len() + len > target {
                    return Err("snappy data exceeds its declared length".into());
                }
                out.extend_from_slice(lit);
                i += len;
                continue;
            }
            1 => {
                let next = *b.get(i).ok_or("truncated snappy copy")?;
                i += 1;
                (
                    4 + usize::from((tag >> 2) & 7),
                    usize::from(tag >> 5) << 8 | usize::from(next),
                )
            }
            2 => {
                let o = b.get(i..i + 2).ok_or("truncated snappy copy")?;
                i += 2;
                (
                    1 + usize::from(tag >> 2),
                    usize::from(u16::from_le_bytes([o[0], o[1]])),
                )
            }
            _ => {
                let o = b.get(i..i + 4).ok_or("truncated snappy copy")?;
                i += 4;
                (
                    1 + usize::from(tag >> 2),
                    u32::from_le_bytes([o[0], o[1], o[2], o[3]]) as usize,
                )
            }
        };
        let produced = out.len() - base;
        if offset == 0 || offset > produced {
            return Err("invalid snappy copy offset".into());
        }
        if out.len() + len > target {
            return Err("snappy data exceeds its declared length".into());
        }
        let start = out.len() - offset;
        if offset >= len {
            out.extend_from_within(start..start + len);
        } else {
            // overlapping copy (run-length style): byte by byte
            for k in 0..len {
                let byte = out[start + k];
                out.push(byte);
            }
        }
    }
    if out.len() != target {
        return Err("snappy data is shorter than its declared length".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "abcabcabcabc" + "XYZ": literal "abc", overlapping 1-byte-offset copy of 9, literal.
    fn sample_block() -> Vec<u8> {
        let mut b = vec![15]; // uncompressed length
        b.push(((3 - 1) << 2) as u8); // literal of 3
        b.extend_from_slice(b"abc");
        b.push(((9 - 4) << 2 | 1) as u8); // copy-1, len 9
        b.push(3); // offset 3
        b.push(((3 - 1) << 2) as u8);
        b.extend_from_slice(b"XYZ");
        b
    }

    #[test]
    fn raw_blocks() {
        assert_eq!(
            decompress(&sample_block(), 1 << 20).unwrap(),
            b"abcabcabcabcXYZ"
        );
        // copy-2 and copy-4 elements, and a long literal (length in an extra byte)
        let lit: Vec<u8> = (0..100u8).collect();
        let mut b = vec![200, 1]; // varint 200
        b.push(60 << 2); // literal, 1 extra length byte
        b.push(99);
        b.extend_from_slice(&lit);
        b.push(((50 - 1) << 2 | 2) as u8);
        b.extend_from_slice(&100u16.to_le_bytes());
        b.push(((50 - 1) << 2 | 3) as u8);
        b.extend_from_slice(&125u32.to_le_bytes());
        let out = decompress(&b, 1 << 20).unwrap();
        assert_eq!(out.len(), 200);
        assert_eq!(&out[..100], &lit[..]);
        assert_eq!(&out[100..150], &lit[..50]);
        assert_eq!(&out[150..], &lit[25..75]);
        assert_eq!(decompress(&[0], 10).unwrap(), b"");
    }

    #[test]
    fn framed_streams() {
        let mut s = STREAM_ID.to_vec();
        let block = sample_block();
        s.push(0x00);
        s.extend_from_slice(&((block.len() + 4) as u32).to_le_bytes()[..3]);
        s.extend_from_slice(&[0; 4]); // CRC (not verified)
        s.extend_from_slice(&block);
        s.push(0x01);
        s.extend_from_slice(&(4u32 + 2).to_le_bytes()[..3]);
        s.extend_from_slice(&[0; 4]);
        s.extend_from_slice(b"!!");
        s.extend_from_slice(&[0xfe, 1, 0, 0, 0]); // padding
        assert_eq!(decompress(&s, 1 << 20).unwrap(), b"abcabcabcabcXYZ!!");
        let mut bad = STREAM_ID.to_vec();
        bad.extend_from_slice(&[0x02, 0, 0, 0]);
        assert!(decompress(&bad, 1 << 20).is_err());
    }

    #[test]
    fn corrupt_and_hostile_input() {
        // bad offsets, truncations, length mismatches, size cap
        assert!(decompress(&[3, 1 << 2 | 1, 5], 100).is_err()); // copy before any output
        let block = sample_block();
        for n in 1..block.len() {
            assert!(decompress(&block[..n], 100).is_err(), "truncated at {n}");
        }
        let mut long = block.clone();
        long[0] = 14; // declared shorter than the data
        assert!(decompress(&long, 100).is_err());
        assert!(
            decompress(&block, 10)
                .unwrap_err()
                .to_string()
                .contains("exceeds 10 bytes")
        );
        // a huge declared length is rejected up front, not allocated
        assert!(decompress(&[0xff, 0xff, 0xff, 0xff, 0x0f], 1 << 20).is_err());
        // run-length bomb within the cap still works and is bounded by `max`
        let mut rle = vec![0x80, 0x80, 0x04]; // 65536
        rle.push(0);
        rle.push(b'a');
        for _ in 0..1092 {
            rle.push((59 << 2 | 2) as u8); // copy-2 len 60, offset 1
            rle.extend_from_slice(&1u16.to_le_bytes());
        }
        rle.push(((15 - 1) << 2 | 2) as u8); // remaining 65536 - 1 - 1092*60 = 15
        rle.extend_from_slice(&1u16.to_le_bytes());
        assert_eq!(decompress(&rle, 1 << 20).unwrap(), vec![b'a'; 65536]);
        assert!(decompress(&rle, 1000).is_err());
    }
}
