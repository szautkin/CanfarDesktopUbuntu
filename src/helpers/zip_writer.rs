//! A store-only ZIP writer that STREAMS to disk, with ZIP64 where it is needed.
//!
//! The research bundle used to be assembled in a `Vec<u8>` with 32-bit size
//! fields, which is fine for its JSON and markdown — kilobytes — and unusable
//! for the observations' FITS files: a multi-gigabyte member either exhausts RAM
//! or wraps the size field and yields an archive that unpacks to garbage,
//! discovered only when someone opens it. That is why the export's
//! `includeFiles` option was refused rather than attempted.
//!
//! Two decisions worth stating:
//!
//! * **Stored, never deflated.** A FITS cube is already the compressed form
//!   (`.fz`) or is floating-point noise that deflate cannot help; spending
//!   minutes of CPU to save nothing is worse than the honest copy. The bundle's
//!   text members are small enough that it makes no difference either way.
//! * **ZIP64 only when a member needs it.** An archive of a few kilobytes of
//!   JSON stays byte-for-byte what it was before this module existed, so the
//!   common export cannot regress for a capability it never uses.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Sizes at or above this cannot be expressed in a 32-bit field, so the entry
/// needs a ZIP64 extra record. Overridable in tests: creating a real 4 GiB
/// member to exercise the ZIP64 path is not a test anyone would run.
const ZIP64_THRESHOLD: u64 = u32::MAX as u64;

/// Streaming writer for a store-only ZIP archive.
pub struct ZipWriter {
    out: BufWriter<File>,
    /// Bytes written so far — the offset the next local header lands at.
    offset: u64,
    central: Vec<u8>,
    count: u64,
    dos_time: u16,
    dos_date: u16,
    zip64_threshold: u64,
    /// Where the most recent central-directory record starts, so a streamed
    /// member's CRC can be patched into it once the bytes have been read.
    last_central_start: Option<usize>,
}

impl ZipWriter {
    /// Create (or truncate) the archive at `path`.
    pub fn create(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let file = File::create(path).map_err(|e| e.to_string())?;
        let (dos_time, dos_date) = dos_datetime_now();
        Ok(ZipWriter {
            out: BufWriter::new(file),
            offset: 0,
            central: Vec::new(),
            count: 0,
            dos_time,
            dos_date,
            zip64_threshold: ZIP64_THRESHOLD,
            last_central_start: None,
        })
    }

    /// Add an in-memory member (the manifest, the READMEs, the JSON).
    pub fn add_bytes(&mut self, name: &str, data: &[u8]) -> Result<(), String> {
        let crc = crc32(data);
        let size = data.len() as u64;
        self.write_entry(name, size, crc, |out| {
            out.write_all(data).map_err(|e| e.to_string())
        })
    }

    /// Add a member copied from a file on disk, streaming it in chunks.
    ///
    /// The size comes from the file's metadata, so only the CRC is unknown when
    /// the local header is written; it is patched in place afterwards. That
    /// keeps this to ONE pass over a file that may be gigabytes — the
    /// alternative, reading it once to checksum and again to copy, doubles the
    /// I/O for every export.
    pub fn add_file(&mut self, name: &str, source: &Path) -> Result<u64, String> {
        let size = std::fs::metadata(source)
            .map_err(|e| format!("{}: {e}", source.display()))?
            .len();

        // Where the CRC field sits inside the local header, so it can be patched
        // once the bytes have been read.
        let crc_offset = self.offset + 14;
        let mut file = File::open(source).map_err(|e| format!("{}: {e}", source.display()))?;
        let mut crc = CRC_INIT;
        let mut copied = 0u64;

        self.write_entry(name, size, 0, |out| {
            let mut buffer = vec![0u8; 256 * 1024];
            loop {
                let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
                if read == 0 {
                    break;
                }
                crc = crc32_update(crc, &buffer[..read]);
                copied += read as u64;
                out.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
            }
            Ok(())
        })?;

        // A file that changed size under us would leave the archive claiming a
        // length it does not contain — an archive that unpacks to garbage, which
        // is the whole failure this module exists to avoid.
        if copied != size {
            return Err(format!(
                "{} changed size while being packed ({size} expected, {copied} read)",
                source.display()
            ));
        }

        let crc = crc ^ CRC_INIT;
        self.patch_crc(crc_offset, crc)?;
        // The central directory copy of the CRC was written with the same
        // placeholder; fix it too.
        self.patch_central_crc(crc);
        Ok(size)
    }

    /// Write the central directory and close the archive.
    pub fn finish(mut self) -> Result<(), String> {
        let central_offset = self.offset;
        let central_size = self.central.len() as u64;
        let central = std::mem::take(&mut self.central);
        self.write_all(&central)?;

        let needs_zip64 = self.count > u16::MAX as u64
            || central_size >= self.zip64_threshold
            || central_offset >= self.zip64_threshold;

        if needs_zip64 {
            // ── ZIP64 end of central directory record ──
            let zip64_eocd_offset = self.offset;
            let mut rec = Vec::new();
            push_u32(&mut rec, 0x0606_4b50);
            push_u64(&mut rec, 44); // size of this record, minus the first 12 bytes
            push_u16(&mut rec, 45); // version made by
            push_u16(&mut rec, 45); // version needed to extract
            push_u32(&mut rec, 0); // this disk
            push_u32(&mut rec, 0); // disk with the central directory
            push_u64(&mut rec, self.count);
            push_u64(&mut rec, self.count);
            push_u64(&mut rec, central_size);
            push_u64(&mut rec, central_offset);
            self.write_all(&rec)?;

            // ── ZIP64 locator ──
            let mut loc = Vec::new();
            push_u32(&mut loc, 0x0706_4b50);
            push_u32(&mut loc, 0); // disk with the ZIP64 EOCD
            push_u64(&mut loc, zip64_eocd_offset);
            push_u32(&mut loc, 1); // total disks
            self.write_all(&loc)?;
        }

        // ── End of central directory record ──
        //
        // The sentinels tell a reader to consult the ZIP64 record instead; a
        // reader that predates ZIP64 sees a full-looking archive it cannot
        // address, which is exactly what the format intends.
        let mut eocd = Vec::new();
        push_u32(&mut eocd, 0x0605_4b50);
        push_u16(&mut eocd, 0); // number of this disk
        push_u16(&mut eocd, 0); // disk where the central directory starts
        push_u16(&mut eocd, cap_u16(self.count));
        push_u16(&mut eocd, cap_u16(self.count));
        push_u32(&mut eocd, cap_u32(central_size, self.zip64_threshold));
        push_u32(&mut eocd, cap_u32(central_offset, self.zip64_threshold));
        push_u16(&mut eocd, 0); // ZIP file comment length
        self.write_all(&eocd)?;

        self.out.flush().map_err(|e| e.to_string())
    }

    // ── internals ───────────────────────────────────────────────────────────

    /// Write one member: local header, then its bytes via `write_data`.
    fn write_entry<F>(
        &mut self,
        name: &str,
        size: u64,
        crc: u32,
        write_data: F,
    ) -> Result<(), String>
    where
        F: FnOnce(&mut BufWriter<File>) -> Result<(), String>,
    {
        let name_bytes = name.as_bytes();
        let offset = self.offset;
        let zip64 = size >= self.zip64_threshold;
        let version = if zip64 { 45 } else { 20 };

        let mut header = Vec::with_capacity(30 + name_bytes.len() + 20);
        push_u32(&mut header, 0x0403_4b50);
        push_u16(&mut header, version);
        push_u16(&mut header, 0); // general-purpose bit flag
        push_u16(&mut header, 0); // compression method: 0 = stored
        push_u16(&mut header, self.dos_time);
        push_u16(&mut header, self.dos_date);
        push_u32(&mut header, crc);
        push_u32(&mut header, cap_u32(size, self.zip64_threshold)); // compressed
        push_u32(&mut header, cap_u32(size, self.zip64_threshold)); // uncompressed
        push_u16(&mut header, name_bytes.len() as u16);
        push_u16(&mut header, if zip64 { 20 } else { 0 }); // extra field length
        header.extend_from_slice(name_bytes);
        if zip64 {
            push_u16(&mut header, 0x0001); // ZIP64 extended information
            push_u16(&mut header, 16); // 2 × u64
            push_u64(&mut header, size); // uncompressed
            push_u64(&mut header, size); // compressed
        }
        self.write_all(&header)?;

        let before = self.offset;
        write_data(&mut self.out)?;
        self.offset = before + size;

        self.push_central(name_bytes, size, crc, offset, zip64);
        self.count += 1;
        Ok(())
    }

    fn push_central(&mut self, name: &[u8], size: u64, crc: u32, offset: u64, zip64: bool) {
        let big_offset = offset >= self.zip64_threshold;
        let extra_len = if zip64 || big_offset {
            4 + if zip64 { 16 } else { 0 } + if big_offset { 8 } else { 0 }
        } else {
            0
        };
        let version = if zip64 || big_offset { 45 } else { 20 };

        self.last_central_start = Some(self.central.len());
        push_u32(&mut self.central, 0x0201_4b50);
        push_u16(&mut self.central, version); // version made by
        push_u16(&mut self.central, version); // version needed to extract
        push_u16(&mut self.central, 0); // general-purpose bit flag
        push_u16(&mut self.central, 0); // compression method
        push_u16(&mut self.central, self.dos_time);
        push_u16(&mut self.central, self.dos_date);
        push_u32(&mut self.central, crc);
        push_u32(&mut self.central, cap_u32(size, self.zip64_threshold)); // compressed
        push_u32(&mut self.central, cap_u32(size, self.zip64_threshold)); // uncompressed
        push_u16(&mut self.central, name.len() as u16);
        push_u16(&mut self.central, extra_len as u16);
        push_u16(&mut self.central, 0); // file comment length
        push_u16(&mut self.central, 0); // disk number start
        push_u16(&mut self.central, 0); // internal file attributes
        push_u32(&mut self.central, 0); // external file attributes
        push_u32(&mut self.central, cap_u32(offset, self.zip64_threshold));
        self.central.extend_from_slice(name);
        if extra_len > 0 {
            push_u16(&mut self.central, 0x0001);
            push_u16(&mut self.central, (extra_len - 4) as u16);
            if zip64 {
                push_u64(&mut self.central, size); // uncompressed
                push_u64(&mut self.central, size); // compressed
            }
            if big_offset {
                push_u64(&mut self.central, offset);
            }
        }
    }

    /// Patch the CRC in a local header already on disk, then seek back to the end.
    fn patch_crc(&mut self, crc_offset: u64, crc: u32) -> Result<(), String> {
        self.out.flush().map_err(|e| e.to_string())?;
        let end = self.offset;
        let file = self.out.get_mut();
        file.seek(SeekFrom::Start(crc_offset))
            .map_err(|e| e.to_string())?;
        file.write_all(&crc.to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(end)).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Fix the CRC in the central-directory record just pushed.
    ///
    /// It sits 16 bytes into the record; the record's start is found by walking
    /// back over what `push_central` appended.
    fn patch_central_crc(&mut self, crc: u32) {
        if let Some(start) = self.last_central_start {
            self.central[start + 16..start + 20].copy_from_slice(&crc.to_le_bytes());
        }
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.out.write_all(bytes).map_err(|e| e.to_string())?;
        self.offset += bytes.len() as u64;
        Ok(())
    }
}

/// Cap a value at the 32-bit sentinel when it needs a ZIP64 record.
fn cap_u32(value: u64, threshold: u64) -> u32 {
    if value >= threshold {
        u32::MAX
    } else {
        value as u32
    }
}

fn cap_u16(value: u64) -> u16 {
    if value > u16::MAX as u64 {
        u16::MAX
    } else {
        value as u16
    }
}

#[inline]
fn push_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn push_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

const CRC_INIT: u32 = 0xFFFF_FFFF;

/// Standard IEEE CRC-32, fed incrementally so a streamed member never has to be
/// held in memory to be checksummed.
fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    crc
}

/// CRC-32 of a complete buffer.
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(CRC_INIT, data) ^ CRC_INIT
}

/// Current local time as a (DOS time, DOS date) pair.
fn dos_datetime_now() -> (u16, u16) {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    let time =
        ((now.hour() as u16) << 11) | ((now.minute() as u16) << 5) | (now.second() as u16 / 2);
    let date = (((now.year() - 1980).max(0) as u16) << 9)
        | ((now.month() as u16) << 5)
        | (now.day() as u16);
    (time, date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A unique scratch directory per test, removed on drop.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!(
                "verbinal_zip_{}_{}_{}",
                tag,
                std::process::id(),
                nanos
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch { dir }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Run `unzip -t` over an archive, or `None` when unzip is not installed.
    ///
    /// The point of this module is producing archives OTHER tools accept, and
    /// only another tool can judge that — a self-check would agree with whatever
    /// the writer happens to emit, including a wrong ZIP64 record.
    fn unzip_test(archive: &Path) -> Option<bool> {
        let out = std::process::Command::new("unzip")
            .arg("-t")
            .arg(archive)
            .output()
            .ok()?;
        Some(out.status.success())
    }

    fn unzip_extract(archive: &Path, into: &Path) -> Option<bool> {
        let out = std::process::Command::new("unzip")
            .arg("-q")
            .arg("-o")
            .arg(archive)
            .arg("-d")
            .arg(into)
            .output()
            .ok()?;
        Some(out.status.success())
    }

    #[test]
    fn the_crc32_matches_the_known_check_value() {
        // The standard IEEE check: CRC-32("123456789") = 0xCBF43926. A wrong
        // CRC yields an archive every extractor rejects at the last moment.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn crc_is_the_same_whether_fed_at_once_or_in_pieces() {
        // Streaming a gigabyte member depends on this.
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let whole = crc32(&data);
        let mut running = CRC_INIT;
        for chunk in data.chunks(97) {
            running = crc32_update(running, chunk);
        }
        assert_eq!(running ^ CRC_INIT, whole);
    }

    #[test]
    fn an_archive_of_in_memory_members_is_valid() {
        let scratch = Scratch::new("bytes");
        let archive = scratch.path("bundle.zip");

        let mut zip = ZipWriter::create(&archive).unwrap();
        zip.add_bytes("manifest.json", b"{\"schema\":1}").unwrap();
        zip.add_bytes("README.md", "# Bundle\n".repeat(50).as_bytes())
            .unwrap();
        zip.finish().unwrap();

        assert!(archive.exists());
        if let Some(ok) = unzip_test(&archive) {
            assert!(ok, "unzip rejected the archive");
        }
    }

    #[test]
    fn a_streamed_file_arrives_byte_for_byte() {
        let scratch = Scratch::new("stream");
        let source = scratch.path("data.fits");
        // Larger than the copy buffer, so the chunked path and the incremental
        // CRC are both exercised.
        let payload: Vec<u8> = (0..700_000u32).map(|i| (i % 256) as u8).collect();
        std::fs::write(&source, &payload).unwrap();

        let archive = scratch.path("bundle.zip");
        let mut zip = ZipWriter::create(&archive).unwrap();
        zip.add_bytes("manifest.json", b"{}").unwrap();
        let written = zip.add_file("observations/data.fits", &source).unwrap();
        zip.finish().unwrap();
        assert_eq!(written, payload.len() as u64);

        let Some(ok) = unzip_test(&archive) else {
            return;
        };
        assert!(ok, "unzip rejected the archive (CRC or size mismatch)");

        let out = scratch.path("out");
        assert_eq!(unzip_extract(&archive, &out), Some(true));
        let extracted = std::fs::read(out.join("observations/data.fits")).unwrap();
        assert_eq!(
            extracted, payload,
            "the member did not survive the round trip"
        );
    }

    #[test]
    fn a_member_over_the_threshold_takes_the_zip64_path() {
        // The threshold is lowered rather than writing a 4 GiB file: what needs
        // testing is the ZIP64 LAYOUT, and unzip judges that just as well on a
        // small member.
        let scratch = Scratch::new("zip64");
        let source = scratch.path("big.fits");
        let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 97) as u8).collect();
        std::fs::write(&source, &payload).unwrap();

        let archive = scratch.path("bundle.zip");
        let mut zip = ZipWriter::create(&archive).unwrap();
        zip.zip64_threshold = 1024;
        zip.add_bytes("small.json", b"{}").unwrap();
        zip.add_file("observations/big.fits", &source).unwrap();
        zip.finish().unwrap();

        let raw = std::fs::read(&archive).unwrap();
        assert!(
            raw.windows(4).any(|w| w == 0x0606_4b50u32.to_le_bytes()),
            "no ZIP64 end-of-central-directory record"
        );
        assert!(
            raw.windows(4).any(|w| w == 0x0706_4b50u32.to_le_bytes()),
            "no ZIP64 locator"
        );

        let Some(ok) = unzip_test(&archive) else {
            return;
        };
        assert!(ok, "unzip rejected the ZIP64 archive");

        let out = scratch.path("out");
        assert_eq!(unzip_extract(&archive, &out), Some(true));
        assert_eq!(
            std::fs::read(out.join("observations/big.fits")).unwrap(),
            payload
        );
    }

    #[test]
    fn a_small_archive_stays_out_of_zip64() {
        // The common export must not change shape for a capability it does not
        // use: a reader that predates ZIP64 still has to open it.
        let scratch = Scratch::new("plain");
        let archive = scratch.path("bundle.zip");
        let mut zip = ZipWriter::create(&archive).unwrap();
        zip.add_bytes("manifest.json", b"{}").unwrap();
        zip.finish().unwrap();

        let raw = std::fs::read(&archive).unwrap();
        assert!(
            !raw.windows(4).any(|w| w == 0x0606_4b50u32.to_le_bytes()),
            "a kilobyte archive should carry no ZIP64 record"
        );
    }

    #[test]
    fn a_missing_source_file_is_an_error_not_a_truncated_member() {
        let scratch = Scratch::new("missing");
        let archive = scratch.path("bundle.zip");
        let mut zip = ZipWriter::create(&archive).unwrap();
        let err = zip
            .add_file("observations/gone.fits", &scratch.path("gone.fits"))
            .unwrap_err();
        assert!(err.contains("gone.fits"), "{err}");
    }
}
