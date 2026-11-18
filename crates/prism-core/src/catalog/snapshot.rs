//! The snapshot file of the file store (`file-index.bin`).
//!
//! ```text
//! magic (8 bytes) | format version (u32)
//! section: header   = volume count (u32)
//! per volume, 13 sections:
//!   meta, lowercase name text, name ends, name heads,
//!   variant text, variant ends, variant lower numbers,
//!   parent, display, flags, seq, next, by_record
//! section: trailer  = "PRISM-END"
//! ```
//!
//! A section is its length (u64), its bytes, and the CRC-32 of the bytes (u32).
//! A case variant is an exact-case name that differs from its lowercase name.
//! The "display" section holds, for each entry, either the lowercase name
//! number (the exact name equals it) or `0x8000_0000` | the variant number.
//! Versions 2 and 3 had a display table with one name for each exact-case
//! name (the same three sections, an empty range for a name equal to its
//! lowercase form) and a display number for each entry. The load converts it
//! to case variants. Version 2 also had a 14th section, "places", after the
//! flags. The load reads and drops it. So an update does not force a full
//! rebuild. All numbers are little endian. The name tables, the masks and the hash
//! tables are not stored: the load builds them again from the text.
//!
//! The store is first serialized to memory (`serialize`, under the store
//! lock). The bytes are then written to `<path>.tmp`, flushed to disk, and
//! renamed over the old file (`write_file`, with no lock), so a crash leaves
//! the old file whole. The load checks every
//! checksum, every length and every index: a problem is an `Err`, and the
//! caller builds the store again.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use super::store::{Names, Segment, Variants, VolumeBackend, VolumeMeta};

const MAGIC: &[u8; 8] = b"PRISMIDX";
const VERSION: u32 = 4;
/// The last version with a display table (see the format above).
const LAST_DISPLAY_TABLE_VERSION: u32 = 3;
/// The last version with the "places" section.
const VERSION_WITH_PLACES: u32 = 2;
const TRAILER: &[u8] = b"PRISM-END";
/// Elements converted to bytes at once.
const CHUNK: usize = 1 << 14;

const CRC_TABLES: [[u32; 256]; 8] = crc_tables();

const fn crc_tables() -> [[u32; 256]; 8] {
    let mut tables = [[0u32; 256]; 8];
    let mut byte = 0;
    while byte < 256 {
        let mut crc = byte as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
            bit += 1;
        }
        tables[0][byte] = crc;
        byte += 1;
    }
    let mut byte = 0;
    while byte < 256 {
        let mut crc = tables[0][byte];
        let mut table = 1;
        while table < 8 {
            crc = tables[0][(crc & 0xff) as usize] ^ (crc >> 8);
            tables[table][byte] = crc;
            table += 1;
        }
        byte += 1;
    }
    tables
}

/// CRC-32 (the zip and PNG polynomial), eight bytes at a time.
struct Crc(u32);

impl Crc {
    fn new() -> Self {
        Self(!0)
    }

    fn update(&mut self, data: &[u8]) {
        let t = &CRC_TABLES;
        let mut crc = self.0;
        let (words, rest) = data.as_chunks::<8>();
        for word in words {
            let low = u32::from_le_bytes([word[0], word[1], word[2], word[3]]) ^ crc;
            let high = u32::from_le_bytes([word[4], word[5], word[6], word[7]]);
            crc = t[7][(low & 0xff) as usize]
                ^ t[6][((low >> 8) & 0xff) as usize]
                ^ t[5][((low >> 16) & 0xff) as usize]
                ^ t[4][(low >> 24) as usize]
                ^ t[3][(high & 0xff) as usize]
                ^ t[2][((high >> 8) & 0xff) as usize]
                ^ t[1][((high >> 16) & 0xff) as usize]
                ^ t[0][(high >> 24) as usize];
        }
        for &byte in rest {
            crc = t[0][((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8);
        }
        self.0 = crc;
    }

    fn finish(&self) -> u32 {
        !self.0
    }
}

struct Out {
    buffer: Vec<u8>,
    scratch: Vec<u8>,
}

impl Out {
    fn raw(&mut self, data: &[u8]) -> Result<(), String> {
        self.buffer.extend_from_slice(data);
        Ok(())
    }

    /// A section of `length` bytes that `fill` writes through `put`.
    fn section(
        &mut self,
        length: u64,
        fill: impl FnOnce(&mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<(), String>,
    ) -> Result<(), String> {
        self.raw(&length.to_le_bytes())?;
        let mut crc = Crc::new();
        let mut written = 0u64;
        {
            let mut put = |data: &[u8]| -> Result<(), String> {
                crc.update(data);
                written += data.len() as u64;
                self.buffer.extend_from_slice(data);
                Ok(())
            };
            fill(&mut put)?;
        }
        if written != length {
            return Err("a section has the wrong length".into());
        }
        self.raw(&crc.finish().to_le_bytes())
    }

    fn bytes_section(&mut self, data: &[u8]) -> Result<(), String> {
        self.section(data.len() as u64, |put| put(data))
    }

    fn u32_section(&mut self, data: &[u32]) -> Result<(), String> {
        let mut scratch = std::mem::take(&mut self.scratch);
        let result = self.section(data.len() as u64 * 4, |put| {
            for chunk in data.chunks(CHUNK) {
                scratch.clear();
                for value in chunk {
                    scratch.extend_from_slice(&value.to_le_bytes());
                }
                put(&scratch)?;
            }
            Ok(())
        });
        self.scratch = scratch;
        result
    }

    fn u16_section(&mut self, data: &[u16]) -> Result<(), String> {
        let mut scratch = std::mem::take(&mut self.scratch);
        let result = self.section(data.len() as u64 * 2, |put| {
            for chunk in data.chunks(CHUNK) {
                scratch.clear();
                for value in chunk {
                    scratch.extend_from_slice(&value.to_le_bytes());
                }
                put(&scratch)?;
            }
            Ok(())
        });
        self.scratch = scratch;
        result
    }
}

fn put_text(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
}

fn meta_bytes(segment: &Segment) -> Vec<u8> {
    let meta = &segment.meta;
    let mut out = Vec::new();
    put_text(&mut out, &meta.volume_id);
    put_text(&mut out, &meta.mount_path);
    out.extend_from_slice(&meta.root_record.to_le_bytes());
    out.push(match meta.backend {
        VolumeBackend::Journal => 0,
        VolumeBackend::Walk => 1,
    });
    out.extend_from_slice(&meta.journal_id.to_le_bytes());
    out.extend_from_slice(&meta.next_usn.to_le_bytes());
    out.extend_from_slice(&meta.exclusions.to_le_bytes());
    out
}

/// Writes the snapshot in the old layout of version 2 (display table and
/// places).
#[cfg(test)]
pub(super) fn write_version_2(path: &Path, segments: &[&Segment]) -> Result<u64, String> {
    write_file(path, &serialize_legacy(segments, VERSION_WITH_PLACES)?)
}

/// Writes the snapshot in the old layout of version 3 (display table).
#[cfg(test)]
pub(super) fn write_version_3(path: &Path, segments: &[&Segment]) -> Result<u64, String> {
    write_file(path, &serialize_legacy(segments, LAST_DISPLAY_TABLE_VERSION)?)
}

/// Writes the snapshot. Returns its size in bytes.
#[cfg(test)]
pub(super) fn write(path: &Path, segments: &[&Segment]) -> Result<u64, String> {
    write_file(path, &serialize(segments)?)
}

/// The bytes of the snapshot file. Memory only: the caller can hold a lock
/// while it runs and release it before `write_file`.
pub(super) fn serialize(segments: &[&Segment]) -> Result<Vec<u8>, String> {
    let mut out = Out { buffer: Vec::new(), scratch: Vec::new() };
    out.raw(MAGIC)?;
    out.raw(&VERSION.to_le_bytes())?;
    out.bytes_section(&(segments.len() as u32).to_le_bytes())?;
    for segment in segments {
        out.bytes_section(&meta_bytes(segment))?;
        out.bytes_section(segment.names.text.as_bytes())?;
        out.u32_section(&segment.names.ends)?;
        out.u32_section(&segment.names.heads)?;
        out.bytes_section(segment.variants.text.as_bytes())?;
        out.u32_section(&segment.variants.ends)?;
        out.u32_section(&segment.variants.lower)?;
        out.u32_section(&segment.parent)?;
        out.u32_section(&segment.display)?;
        out.u16_section(&segment.flags)?;
        out.u16_section(&segment.seq)?;
        out.u32_section(&segment.next)?;
        out.u32_section(&segment.by_record)?;
    }
    out.bytes_section(TRAILER)?;
    Ok(out.buffer)
}

/// `serialize` in the layout of version 2 or 3: a display table with one
/// name for each exact-case name that an entry uses, and a display number
/// for each entry. Only the tests write the old layouts.
#[cfg(test)]
fn serialize_legacy(segments: &[&Segment], version: u32) -> Result<Vec<u8>, String> {
    let mut out = Out { buffer: Vec::new(), scratch: Vec::new() };
    out.raw(MAGIC)?;
    out.raw(&version.to_le_bytes())?;
    out.bytes_section(&(segments.len() as u32).to_le_bytes())?;
    for segment in segments {
        // The old display numbers, in the order that the entries first use them.
        let mut numbers = std::collections::HashMap::new();
        let mut text = String::new();
        let mut ends = Vec::new();
        let mut lower = Vec::new();
        let mut display = Vec::with_capacity(segment.display.len());
        for (entry, &value) in segment.display.iter().enumerate() {
            if entry == 0 {
                display.push(u32::MAX);
                continue;
            }
            let number = *numbers.entry(value).or_insert_with(|| {
                let number = ends.len() as u32;
                if value & 0x8000_0000 == 0 {
                    // Equal to its lowercase name: an empty range.
                    lower.push(value);
                } else {
                    let variant = (value & !0x8000_0000) as usize;
                    let start = if variant == 0 { 0 } else { segment.variants.ends[variant - 1] };
                    text.push_str(&segment.variants.text[start as usize..segment.variants.ends[variant] as usize]);
                    lower.push(segment.variants.lower[variant]);
                }
                ends.push(text.len() as u32);
                number
            });
            display.push(number);
        }
        out.bytes_section(&meta_bytes(segment))?;
        out.bytes_section(segment.names.text.as_bytes())?;
        out.u32_section(&segment.names.ends)?;
        out.u32_section(&segment.names.heads)?;
        out.bytes_section(text.as_bytes())?;
        out.u32_section(&ends)?;
        out.u32_section(&lower)?;
        out.u32_section(&segment.parent)?;
        out.u32_section(&display)?;
        out.u16_section(&segment.flags)?;
        if version == VERSION_WITH_PLACES {
            let places: Vec<u32> = (0..segment.entries() as u32).map(|e| segment.places_of(e)).collect();
            out.u32_section(&places)?;
        }
        out.u16_section(&segment.seq)?;
        out.u32_section(&segment.next)?;
        out.u32_section(&segment.by_record)?;
    }
    out.bytes_section(TRAILER)?;
    Ok(out.buffer)
}

/// Writes `bytes` to `<path>.tmp`, flushes it to disk, and renames it over
/// `path`. Returns the size in bytes.
pub(super) fn write_file(path: &Path, bytes: &[u8]) -> Result<u64, String> {
    crate::win32::write_atomic_synced(path, bytes)?;
    Ok(bytes.len() as u64)
}

struct In {
    reader: BufReader<File>,
    /// Bytes of the file that are not read yet.
    remaining: u64,
}

impl In {
    fn exact(&mut self, buffer: &mut [u8]) -> Result<(), String> {
        if buffer.len() as u64 > self.remaining {
            return Err("the file is truncated".into());
        }
        self.reader
            .read_exact(buffer)
            .map_err(|e| format!("the file is truncated: {e}"))?;
        self.remaining -= buffer.len() as u64;
        Ok(())
    }

    /// The bytes of the next section, after the checksum test.
    fn section(&mut self) -> Result<Vec<u8>, String> {
        let mut length = [0u8; 8];
        self.exact(&mut length)?;
        let length = u64::from_le_bytes(length);
        // The checksum follows: a length past the file is a damaged header.
        if length.checked_add(4).is_none_or(|needed| needed > self.remaining) {
            return Err("a section is longer than the file".into());
        }
        let mut bytes = vec![0u8; length as usize];
        self.exact(&mut bytes)?;
        let mut stored = [0u8; 4];
        self.exact(&mut stored)?;
        let mut crc = Crc::new();
        crc.update(&bytes);
        if crc.finish() != u32::from_le_bytes(stored) {
            return Err("a checksum does not match".into());
        }
        Ok(bytes)
    }

    fn text(&mut self) -> Result<String, String> {
        String::from_utf8(self.section()?).map_err(|_| "a text section is not UTF-8".to_string())
    }

    fn u32s(&mut self) -> Result<Vec<u32>, String> {
        let bytes = self.section()?;
        if !bytes.len().is_multiple_of(4) {
            return Err("a number section has a wrong length".into());
        }
        Ok(bytes.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect())
    }

    fn u16s(&mut self) -> Result<Vec<u16>, String> {
        let bytes = self.section()?;
        if !bytes.len().is_multiple_of(2) {
            return Err("a number section has a wrong length".into());
        }
        Ok(bytes.as_chunks::<2>().0.iter().map(|word| u16::from_le_bytes(*word)).collect())
    }
}

/// A cursor over the bytes of a meta section.
struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        if count > self.0.len() {
            return Err("the volume header is short".into());
        }
        let (head, tail) = self.0.split_at(count);
        self.0 = tail;
        Ok(head)
    }

    fn u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| "short")?))
    }

    fn text(&mut self) -> Result<String, String> {
        let length = self.take(4)?;
        let length = u32::from_le_bytes([length[0], length[1], length[2], length[3]]) as usize;
        String::from_utf8(self.take(length)?.to_vec())
            .map_err(|_| "the volume header is not UTF-8".to_string())
    }
}

fn parse_meta(bytes: &[u8]) -> Result<VolumeMeta, String> {
    let mut cursor = Cursor(bytes);
    let volume_id = cursor.text()?;
    let mount_path = cursor.text()?;
    let root_record = cursor.u64()?;
    let backend = match cursor.take(1)?[0] {
        0 => VolumeBackend::Journal,
        1 => VolumeBackend::Walk,
        _ => return Err("an unknown volume backend".into()),
    };
    let journal_id = cursor.u64()?;
    let next_usn = cursor.u64()? as i64;
    let exclusions = cursor.u64()?;
    if !cursor.0.is_empty() {
        return Err("the volume header has a tail".into());
    }
    Ok(VolumeMeta { volume_id, mount_path, root_record, backend, journal_id, next_usn, exclusions })
}

/// Reads a snapshot into segments (their ids and slots are the caller's).
pub(super) fn read(path: &Path) -> Result<Vec<Segment>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let remaining = file.metadata().map_err(|e| e.to_string())?.len();
    let mut input = In { reader: BufReader::with_capacity(1 << 20, file), remaining };
    let mut magic = [0u8; 8];
    input.exact(&mut magic)?;
    if &magic != MAGIC {
        return Err("not a file index".into());
    }
    let mut version = [0u8; 4];
    input.exact(&mut version)?;
    let version = u32::from_le_bytes(version);
    if !(VERSION_WITH_PLACES..=VERSION).contains(&version) {
        return Err("an unknown file index version".into());
    }
    let header = input.section()?;
    let count = u32::from_le_bytes(
        header.as_slice().try_into().map_err(|_| "the header has a wrong length".to_string())?,
    );
    let mut segments = Vec::new();
    for _ in 0..count {
        let meta = parse_meta(&input.section()?)?;
        let mut names = Names::default();
        names.text = input.text()?;
        names.ends = input.u32s()?;
        names.heads = input.u32s()?;
        let mut variants = Variants::default();
        variants.text = input.text()?;
        variants.ends = input.u32s()?;
        variants.lower = input.u32s()?;
        let parent = input.u32s()?;
        let mut display = input.u32s()?;
        if version <= LAST_DISPLAY_TABLE_VERSION {
            // The sections held a display table: convert it to case variants
            // and give each entry its new display value.
            let (converted, map) = Variants::from_legacy(
                &variants.text,
                &variants.ends,
                &variants.lower,
                names.ends.len(),
            )?;
            variants = converted;
            for value in display.iter_mut().skip(1) {
                *value = *map
                    .get(*value as usize)
                    .ok_or("an entry's name points outside its array")?;
            }
        }
        let flags = input.u16s()?;
        if version == VERSION_WITH_PLACES {
            // The entry count must still match; the values are not kept.
            if input.section()?.len() != parent.len() * 4 {
                return Err("the entry arrays differ in length".into());
            }
        }
        let seq = input.u16s()?;
        let next = input.u32s()?;
        let by_record = input.u32s()?;
        segments.push(Segment::from_parts(
            0, meta, names, variants, parent, display, flags, seq, next, by_record,
        )?);
    }
    if input.section()? != TRAILER {
        return Err("the trailer is wrong".into());
    }
    if input.remaining != 0 {
        return Err("the file has bytes after the trailer".into());
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_known_value() {
        let mut crc = Crc::new();
        crc.update(b"123456789");
        assert_eq!(crc.finish(), 0xCBF4_3926);
        // The same in pieces, across the eight-byte step.
        let mut crc = Crc::new();
        crc.update(b"1234");
        crc.update(b"56789");
        assert_eq!(crc.finish(), 0xCBF4_3926);
        assert_eq!(Crc::new().finish(), 0);
    }
}
