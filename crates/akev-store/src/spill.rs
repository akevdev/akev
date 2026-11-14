//! Disk spill for body bytes that no longer fit the in-memory budget.
//!
//! Bodies are zstd-compressed (level 3; stored raw when compression doesn't help) and
//! appended to anonymous temp files ("segments", ~64 MiB each). Every blob is addressed by
//! a [`SpillHandle`] `(segment, offset, len)`. Reads and writes use positioned I/O
//! (`pread`/`pwrite`), so any number of readers and writers proceed concurrently without a
//! shared file cursor; the internal mutex only guards segment bookkeeping and is never held
//! during I/O or compression.
//!
//! Space is reclaimed per segment: each segment counts its live blobs, and once that count
//! drops to zero (every exchange that referenced it was deleted, evicted or cleared) the
//! segment is closed and the OS frees the anonymous file. Segment ids are never reused, so a
//! handle copied out of the store before its blob was freed reads an error, never another
//! blob's bytes. Clearing the store therefore frees the spill files, and FIFO eviction
//! (`max_exchanges`) plus the disk cap (`spill_max_bytes`, enforced by the store) keep disk
//! usage bounded on long sessions.

use bytes::Bytes;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Segments are sealed once they reach this size (a single blob may exceed it).
const SEGMENT_TARGET: u64 = 64 * 1024 * 1024;
/// zstd level used for spilled bodies (fast, decent ratio).
const ZSTD_LEVEL: i32 = 3;

/// Location of one spilled blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpillHandle {
    pub segment: u32,
    /// Whether the stored bytes are zstd-compressed (else stored raw).
    pub compressed: bool,
    pub offset: u64,
    /// Bytes occupied in the segment.
    pub stored_len: u64,
    /// Original (uncompressed) length.
    pub raw_len: u64,
}

struct Segment {
    file: Arc<File>,
    /// Bytes reserved so far (the next write offset).
    len: u64,
    /// Live blobs referencing this segment (including reserved, in-flight writes).
    live: u64,
}

#[derive(Default)]
struct SpillState {
    segments: HashMap<u32, Segment>,
    current: Option<u32>,
    next_id: u32,
    warned: bool,
}

pub(crate) struct Spill {
    dir: Option<PathBuf>,
    segment_target: u64,
    state: Mutex<SpillState>,
}

impl Spill {
    pub(crate) fn new(dir: Option<&Path>) -> Self {
        Spill {
            dir: dir.map(Path::to_path_buf),
            segment_target: SEGMENT_TARGET,
            state: Mutex::new(SpillState::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_segment_target(dir: Option<&Path>, segment_target: u64) -> Self {
        Spill {
            segment_target,
            ..Spill::new(dir)
        }
    }

    fn create_file(&self) -> io::Result<File> {
        match &self.dir {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                tempfile::tempfile_in(dir)
            }
            None => tempfile::tempfile(),
        }
    }

    /// Compress and append `data`, returning its handle. The caller owns one "live"
    /// reference and must eventually [`Spill::free`] it.
    pub(crate) fn write(&self, data: &[u8]) -> io::Result<SpillHandle> {
        let compressed = zstd::bulk::compress(data, ZSTD_LEVEL)?;
        let (payload, is_compressed): (&[u8], bool) = if compressed.len() < data.len() {
            (&compressed, true)
        } else {
            (data, false)
        };
        let stored_len = payload.len() as u64;

        let (file, segment, offset) = {
            let mut st = self.state.lock();
            let reuse = st.current.filter(|id| {
                st.segments
                    .get(id)
                    .is_some_and(|s| s.len < self.segment_target)
            });
            let id = match reuse {
                Some(id) => id,
                None => {
                    let file = match self.create_file() {
                        Ok(f) => f,
                        Err(e) => {
                            if !st.warned {
                                st.warned = true;
                                tracing::warn!(
                                    error = %e,
                                    dir = ?self.dir,
                                    "cannot create body spill file; bodies stay in memory"
                                );
                            }
                            return Err(e);
                        }
                    };
                    let id = st.next_id;
                    st.next_id = st.next_id.wrapping_add(1);
                    st.segments.insert(
                        id,
                        Segment {
                            file: Arc::new(file),
                            len: 0,
                            live: 0,
                        },
                    );
                    // The previous active segment is now sealed; drop it if nothing uses it.
                    if let Some(prev) = st.current.replace(id)
                        && st.segments.get(&prev).is_some_and(|s| s.live == 0)
                    {
                        st.segments.remove(&prev);
                    }
                    id
                }
            };
            let Some(seg) = st.segments.get_mut(&id) else {
                return Err(io::Error::other("spill segment vanished"));
            };
            let offset = seg.len;
            seg.len += stored_len;
            seg.live += 1;
            (Arc::clone(&seg.file), id, offset)
        };

        let handle = SpillHandle {
            segment,
            compressed: is_compressed,
            offset,
            stored_len,
            raw_len: data.len() as u64,
        };
        if let Err(e) = write_all_at(&file, payload, offset) {
            self.free(&handle);
            return Err(e);
        }
        Ok(handle)
    }

    /// Read a blob back (decompressing if needed).
    pub(crate) fn read(&self, handle: &SpillHandle) -> io::Result<Bytes> {
        let file = {
            let st = self.state.lock();
            match st.segments.get(&handle.segment) {
                Some(seg) => Arc::clone(&seg.file),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "spill segment freed",
                    ));
                }
            }
        };
        let stored_len = usize::try_from(handle.stored_len).map_err(io::Error::other)?;
        let raw_len = usize::try_from(handle.raw_len).map_err(io::Error::other)?;
        let mut buf = vec![0u8; stored_len];
        read_exact_at(&file, &mut buf, handle.offset)?;
        if !handle.compressed {
            return Ok(Bytes::from(buf));
        }
        let raw = zstd::bulk::decompress(&buf, raw_len)?;
        if raw.len() != raw_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "spilled body has wrong length",
            ));
        }
        Ok(Bytes::from(raw))
    }

    /// Release one live reference. Fully drained segments are closed (freeing the anonymous
    /// file); when that was the active segment, the next write starts a new one.
    pub(crate) fn free(&self, handle: &SpillHandle) {
        let mut st = self.state.lock();
        let Some(seg) = st.segments.get_mut(&handle.segment) else {
            return;
        };
        seg.live = seg.live.saturating_sub(1);
        if seg.live > 0 {
            return;
        }
        st.segments.remove(&handle.segment);
        if st.current == Some(handle.segment) {
            st.current = None;
        }
    }

    /// Bytes occupied on disk (all segments, including the space of freed blobs in segments
    /// that are still in use).
    pub(crate) fn disk_bytes(&self) -> u64 {
        self.state.lock().segments.values().map(|s| s.len).sum()
    }

    /// Number of open segment files.
    #[cfg(test)]
    pub(crate) fn segment_count(&self) -> usize {
        self.state.lock().segments.len()
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(unix)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "short spill read",
                ));
            }
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_write(buf, offset) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "short spill write",
                ));
            }
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn read_exact_at(_file: &File, _buf: &mut [u8], _offset: u64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "positioned reads unsupported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn write_all_at(_file: &File, _buf: &[u8], _offset: u64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "positioned writes unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(len: usize) -> Vec<u8> {
        let mut x: u32 = 0x9e37_79b9;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 8) as u8
            })
            .collect()
    }

    #[test]
    fn write_read_free_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let spill = Spill::new(Some(&dir.path().join("nested/spill")));
        let text = b"hello hello hello hello hello hello hello hello".repeat(100);
        let random = noise(4096);
        let h1 = spill.write(&text).unwrap();
        let h2 = spill.write(&random).unwrap();
        assert!(h1.compressed && h1.stored_len < text.len() as u64);
        assert!(!h2.compressed, "incompressible data is stored raw");
        assert_eq!(spill.read(&h1).unwrap(), Bytes::from(text.clone()));
        assert_eq!(spill.read(&h2).unwrap(), Bytes::from(random.clone()));
        assert!(spill.disk_bytes() > 0);
        spill.free(&h1);
        assert_eq!(spill.read(&h2).unwrap(), Bytes::from(random));
        spill.free(&h2);
        assert_eq!(spill.disk_bytes(), 0, "active segment freed when drained");
        assert_eq!(spill.segment_count(), 0);
        let h3 = spill.write(b"again").unwrap();
        assert_eq!(h3.offset, 0);
        assert_ne!(h3.segment, h2.segment, "segment ids are never reused");
        assert_eq!(&spill.read(&h3).unwrap()[..], b"again");
        // A stale handle to a freed blob errors instead of reading another blob.
        assert!(spill.read(&h1).is_err());
    }

    #[test]
    fn concurrent_reads_and_writes() {
        let spill = Arc::new(Spill::new(None));
        let handles: Vec<_> = (0..8u8)
            .map(|t| {
                let spill = Arc::clone(&spill);
                std::thread::spawn(move || {
                    let mut out = Vec::new();
                    for i in 0..50u32 {
                        let data = vec![t.wrapping_add(i as u8); 100 + i as usize * 7];
                        let h = spill.write(&data).unwrap();
                        out.push((h, data));
                    }
                    for (h, data) in &out {
                        assert_eq!(&spill.read(h).unwrap()[..], &data[..]);
                    }
                    out
                })
            })
            .collect();
        let all: Vec<_> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        for (h, data) in &all {
            assert_eq!(&spill.read(h).unwrap()[..], &data[..]);
        }
        for (h, _) in &all {
            spill.free(h);
        }
        assert_eq!(spill.disk_bytes(), 0);
    }

    #[test]
    fn sealed_segments_are_dropped_when_drained() {
        let spill = Spill::with_segment_target(None, 64);
        // A blob larger than the segment target seals the segment for the next write.
        let h_big = spill.write(&noise(1000)).unwrap();
        let h_small = spill.write(b"small").unwrap();
        assert_ne!(h_big.segment, h_small.segment);
        assert_eq!(spill.segment_count(), 2);
        spill.free(&h_big);
        assert_eq!(
            spill.segment_count(),
            1,
            "sealed segment closed once drained"
        );
        assert_eq!(&spill.read(&h_small).unwrap()[..], b"small");
        assert!(spill.read(&h_big).is_err());
    }
}
