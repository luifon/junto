//! A writer of ZIP files for exports (ADR 0411).
//!
//! Every entry is stored without compression and described in the ZIP64 form, so that a
//! file and the archive can be larger than 4 GiB. The writer holds no content: an entry is
//! written as its bytes arrive, its checksum and its size are written into its header when
//! it ends, and the directory of the archive is collected in a second file until the end.

use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};

use time::OffsetDateTime;

const LOCAL_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const END64_SIGNATURE: u32 = 0x0606_4b50;
const LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const END_SIGNATURE: u32 = 0x0605_4b50;
/// The version that reads the ZIP64 form.
const VERSION: u16 = 45;
/// Made on a system with Unix file modes.
const MADE_BY: u16 = (3 << 8) | VERSION;
/// The names are UTF-8.
const FLAGS: u16 = 0x0800;
const ZIP64_EXTRA: u16 = 0x0001;
/// A regular file that its owner reads and writes and that others read.
const FILE_MODE: u32 = 0o100_644 << 16;
const LOCAL_FIXED: u64 = 30;
const LOCAL_EXTRA: u16 = 20;
const CENTRAL_FIXED: u64 = 46;
const CENTRAL_EXTRA: u16 = 28;
/// The three records at the end of an archive.
pub const END_BYTES: u64 = 56 + 20 + 22;
/// The offset of the checksum in the header of an entry.
const CHECKSUM_AT: u64 = 14;
const BUFFER: usize = 64 * 1024;

/// The bytes that one entry adds to an archive besides its content: its header and its
/// record in the directory.
pub const fn entry_overhead(name_bytes: usize) -> u64 {
    let name = name_bytes as u64;
    LOCAL_FIXED + name + LOCAL_EXTRA as u64 + CENTRAL_FIXED + name + CENTRAL_EXTRA as u64
}

/// The bytes of one record of the directory.
pub const fn directory_record(name_bytes: usize) -> u64 {
    CENTRAL_FIXED + name_bytes as u64 + CENTRAL_EXTRA as u64
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                (value >> 1) ^ 0xEDB8_8320
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// Continues a CRC-32 checksum (the one of ZIP) over `bytes`. A checksum starts at 0.
pub fn crc32(checksum: u32, bytes: &[u8]) -> u32 {
    let mut value = !checksum;
    for byte in bytes {
        value = CRC_TABLE[((value ^ u32::from(*byte)) & 0xFF) as usize] ^ (value >> 8);
    }
    !value
}

/// True for a name that an entry can have: a relative path of parts that are not empty,
/// not `.` and not `..`, without a backslash and without a control character.
pub fn is_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= usize::from(u16::MAX)
        && !name.contains('\\')
        && !name.chars().any(char::is_control)
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn invalid(problem: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, problem)
}

/// The time of an entry in the form of ZIP: two seconds exact, from 1980 to 2107.
fn dos_time(at: OffsetDateTime) -> (u16, u16) {
    let year = u16::try_from(at.year().clamp(1980, 2107) - 1980).unwrap_or(0);
    let date = (year << 9) | (u16::from(u8::from(at.month())) << 5) | u16::from(at.day());
    let time =
        (u16::from(at.hour()) << 11) | (u16::from(at.minute()) << 5) | (u16::from(at.second()) / 2);
    (time, date)
}

struct Open {
    name: Vec<u8>,
    header_at: u64,
    checksum: u32,
    bytes: u64,
}

pub struct ZipWriter {
    file: BufWriter<File>,
    /// The records of the directory, until the end of the archive.
    directory: BufWriter<File>,
    directory_bytes: u64,
    entries: u64,
    /// The bytes that the archive has.
    at: u64,
    open: Option<Open>,
    stamp: (u16, u16),
}

impl ZipWriter {
    /// Writes an archive into `file`, which is empty and can be read. `directory` is an
    /// empty file that the caller deletes afterwards. Every entry has the time `at`.
    pub fn new(file: File, directory: File, at: OffsetDateTime) -> Self {
        Self {
            file: BufWriter::with_capacity(BUFFER, file),
            directory: BufWriter::with_capacity(BUFFER, directory),
            directory_bytes: 0,
            entries: 0,
            at: 0,
            open: None,
            stamp: dos_time(at),
        }
    }

    /// The bytes that the archive has so far, without its directory.
    pub fn bytes(&self) -> u64 {
        self.at
    }

    /// The bytes that the archive and the file of its directory have so far.
    pub fn written(&self) -> u64 {
        self.at + self.directory_bytes
    }

    /// Starts the entry `name`. The entry before it has ended.
    pub fn start(&mut self, name: &str) -> io::Result<()> {
        if self.open.is_some() {
            return Err(invalid("an entry of the archive is still open"));
        }
        if !is_entry_name(name) {
            return Err(invalid("not a name of an entry of an archive"));
        }
        let name = name.as_bytes().to_vec();
        let name_bytes = u16::try_from(name.len()).map_err(|_| invalid("the name is too long"))?;
        let header_at = self.at;
        let mut header = Vec::with_capacity(usize::from(LOCAL_EXTRA) + 30 + name.len());
        header.extend_from_slice(&LOCAL_SIGNATURE.to_le_bytes());
        header.extend_from_slice(&VERSION.to_le_bytes());
        header.extend_from_slice(&FLAGS.to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes());
        header.extend_from_slice(&self.stamp.0.to_le_bytes());
        header.extend_from_slice(&self.stamp.1.to_le_bytes());
        // The checksum and the two sizes are written when the entry ends.
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(&u32::MAX.to_le_bytes());
        header.extend_from_slice(&u32::MAX.to_le_bytes());
        header.extend_from_slice(&name_bytes.to_le_bytes());
        header.extend_from_slice(&LOCAL_EXTRA.to_le_bytes());
        header.extend_from_slice(&name);
        header.extend_from_slice(&ZIP64_EXTRA.to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(&0u64.to_le_bytes());
        header.extend_from_slice(&0u64.to_le_bytes());
        self.file.write_all(&header)?;
        self.at += header.len() as u64;
        self.open = Some(Open {
            name,
            header_at,
            checksum: 0,
            bytes: 0,
        });
        Ok(())
    }

    /// Adds bytes to the open entry.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let open = self
            .open
            .as_mut()
            .ok_or_else(|| invalid("no entry of the archive is open"))?;
        self.file.write_all(bytes)?;
        open.checksum = crc32(open.checksum, bytes);
        open.bytes += bytes.len() as u64;
        self.at += bytes.len() as u64;
        Ok(())
    }

    /// Ends the open entry: writes its checksum and its size into its header and its
    /// record into the directory.
    pub fn end(&mut self) -> io::Result<()> {
        let open = self
            .open
            .take()
            .ok_or_else(|| invalid("no entry of the archive is open"))?;
        let sizes_at = open.header_at + LOCAL_FIXED + open.name.len() as u64 + 4;
        self.file
            .seek(SeekFrom::Start(open.header_at + CHECKSUM_AT))?;
        self.file.write_all(&open.checksum.to_le_bytes())?;
        self.file.seek(SeekFrom::Start(sizes_at))?;
        self.file.write_all(&open.bytes.to_le_bytes())?;
        self.file.write_all(&open.bytes.to_le_bytes())?;
        self.file.seek(SeekFrom::Start(self.at))?;

        let name_bytes =
            u16::try_from(open.name.len()).map_err(|_| invalid("the name is too long"))?;
        let mut record = Vec::with_capacity(46 + open.name.len() + usize::from(CENTRAL_EXTRA));
        record.extend_from_slice(&CENTRAL_SIGNATURE.to_le_bytes());
        record.extend_from_slice(&MADE_BY.to_le_bytes());
        record.extend_from_slice(&VERSION.to_le_bytes());
        record.extend_from_slice(&FLAGS.to_le_bytes());
        record.extend_from_slice(&0u16.to_le_bytes());
        record.extend_from_slice(&self.stamp.0.to_le_bytes());
        record.extend_from_slice(&self.stamp.1.to_le_bytes());
        record.extend_from_slice(&open.checksum.to_le_bytes());
        record.extend_from_slice(&u32::MAX.to_le_bytes());
        record.extend_from_slice(&u32::MAX.to_le_bytes());
        record.extend_from_slice(&name_bytes.to_le_bytes());
        record.extend_from_slice(&CENTRAL_EXTRA.to_le_bytes());
        record.extend_from_slice(&0u16.to_le_bytes());
        record.extend_from_slice(&0u16.to_le_bytes());
        record.extend_from_slice(&0u16.to_le_bytes());
        record.extend_from_slice(&FILE_MODE.to_le_bytes());
        record.extend_from_slice(&u32::MAX.to_le_bytes());
        record.extend_from_slice(&open.name);
        record.extend_from_slice(&ZIP64_EXTRA.to_le_bytes());
        record.extend_from_slice(&24u16.to_le_bytes());
        record.extend_from_slice(&open.bytes.to_le_bytes());
        record.extend_from_slice(&open.bytes.to_le_bytes());
        record.extend_from_slice(&open.header_at.to_le_bytes());
        self.directory.write_all(&record)?;
        self.directory_bytes += record.len() as u64;
        self.entries += 1;
        Ok(())
    }

    /// Writes one whole entry.
    pub fn entry(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        self.start(name)?;
        self.write(bytes)?;
        self.end()
    }

    /// Ends the archive: appends the directory and the end records and writes everything
    /// to the disk. Returns the size of the archive.
    pub fn finish(mut self) -> io::Result<u64> {
        if self.open.is_some() {
            return Err(invalid("an entry of the archive is still open"));
        }
        let directory_at = self.at;
        let mut directory = self
            .directory
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?;
        directory.seek(SeekFrom::Start(0))?;
        let mut buffer = vec![0u8; BUFFER];
        let mut copied = 0u64;
        loop {
            let count = directory.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            self.file.write_all(&buffer[..count])?;
            copied += count as u64;
        }
        if copied != self.directory_bytes {
            return Err(io::Error::other(
                "the directory of the archive is incomplete",
            ));
        }
        let end64_at = directory_at + copied;
        let mut end = Vec::with_capacity(END_BYTES as usize);
        end.extend_from_slice(&END64_SIGNATURE.to_le_bytes());
        end.extend_from_slice(&44u64.to_le_bytes());
        end.extend_from_slice(&MADE_BY.to_le_bytes());
        end.extend_from_slice(&VERSION.to_le_bytes());
        end.extend_from_slice(&0u32.to_le_bytes());
        end.extend_from_slice(&0u32.to_le_bytes());
        end.extend_from_slice(&self.entries.to_le_bytes());
        end.extend_from_slice(&self.entries.to_le_bytes());
        end.extend_from_slice(&copied.to_le_bytes());
        end.extend_from_slice(&directory_at.to_le_bytes());
        end.extend_from_slice(&LOCATOR_SIGNATURE.to_le_bytes());
        end.extend_from_slice(&0u32.to_le_bytes());
        end.extend_from_slice(&end64_at.to_le_bytes());
        end.extend_from_slice(&1u32.to_le_bytes());
        end.extend_from_slice(&END_SIGNATURE.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&u16::MAX.to_le_bytes());
        end.extend_from_slice(&u16::MAX.to_le_bytes());
        end.extend_from_slice(&u32::MAX.to_le_bytes());
        end.extend_from_slice(&u32::MAX.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        self.file.write_all(&end)?;
        let file = self
            .file
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?;
        file.sync_all()?;
        Ok(end64_at + END_BYTES)
    }
}

/// One entry of an archive as [`read`] returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub content: Vec<u8>,
}

fn field<const N: usize>(bytes: &[u8], at: usize) -> io::Result<[u8; N]> {
    bytes
        .get(at..at + N)
        .and_then(|part| part.try_into().ok())
        .ok_or_else(|| io::Error::other("the archive ends early"))
}

fn u16_at(bytes: &[u8], at: usize) -> io::Result<u16> {
    field(bytes, at).map(u16::from_le_bytes)
}

fn u32_at(bytes: &[u8], at: usize) -> io::Result<u32> {
    field(bytes, at).map(u32::from_le_bytes)
}

fn u64_at(bytes: &[u8], at: usize) -> io::Result<u64> {
    field(bytes, at).map(u64::from_le_bytes)
}

fn place(value: u64) -> io::Result<usize> {
    usize::try_from(value).map_err(io::Error::other)
}

/// Reads an archive that [`ZipWriter`] wrote, through its end records and its directory,
/// and checks every checksum. The tests read exports with it; the server reads no archive.
pub fn read(bytes: &[u8]) -> io::Result<Vec<Entry>> {
    let broken = |problem: &'static str| io::Error::other(problem);
    let end = bytes
        .len()
        .checked_sub(22)
        .ok_or_else(|| broken("no end record"))?;
    if u32_at(bytes, end)? != END_SIGNATURE {
        return Err(broken("no end record"));
    }
    let locator = end
        .checked_sub(20)
        .ok_or_else(|| broken("no ZIP64 locator"))?;
    if u32_at(bytes, locator)? != LOCATOR_SIGNATURE {
        return Err(broken("no ZIP64 locator"));
    }
    let end64 = place(u64_at(bytes, locator + 8)?)?;
    if u32_at(bytes, end64)? != END64_SIGNATURE {
        return Err(broken("no ZIP64 end record"));
    }
    let entries = u64_at(bytes, end64 + 32)?;
    let directory_bytes = place(u64_at(bytes, end64 + 40)?)?;
    let mut at = place(u64_at(bytes, end64 + 48)?)?;
    if at + directory_bytes != end64 {
        return Err(broken("the directory does not end at the end record"));
    }
    let mut read = Vec::new();
    for _ in 0..entries {
        if u32_at(bytes, at)? != CENTRAL_SIGNATURE {
            return Err(broken("no record of the directory"));
        }
        let checksum = u32_at(bytes, at + 16)?;
        let name_bytes = usize::from(u16_at(bytes, at + 28)?);
        let extra_bytes = usize::from(u16_at(bytes, at + 30)?);
        let name = bytes
            .get(at + 46..at + 46 + name_bytes)
            .ok_or_else(|| broken("the archive ends early"))?;
        let extra = at + 46 + name_bytes;
        if u16_at(bytes, extra)? != ZIP64_EXTRA || extra_bytes != usize::from(CENTRAL_EXTRA) {
            return Err(broken("a record without the ZIP64 sizes"));
        }
        let size = place(u64_at(bytes, extra + 4)?)?;
        let header = place(u64_at(bytes, extra + 20)?)?;
        if u32_at(bytes, header)? != LOCAL_SIGNATURE
            || u32_at(bytes, header + 14)? != checksum
            || usize::from(u16_at(bytes, header + 26)?) != name_bytes
            || bytes.get(header + 30..header + 30 + name_bytes) != Some(name)
            || place(u64_at(bytes, header + 30 + name_bytes + 4)?)? != size
        {
            return Err(broken("a header that differs from its record"));
        }
        let content_at = header + 30 + name_bytes + usize::from(u16_at(bytes, header + 28)?);
        let content = bytes
            .get(content_at..content_at + size)
            .ok_or_else(|| broken("the archive ends early"))?;
        if crc32(0, content) != checksum {
            return Err(broken("a checksum that differs from its content"));
        }
        read.push(Entry {
            name: String::from_utf8(name.to_vec()).map_err(io::Error::other)?,
            content: content.to_vec(),
        });
        at = extra + extra_bytes;
    }
    if at != end64 {
        return Err(broken("the directory has more records than it states"));
    }
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("junto-zip-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn open(path: &std::path::Path) -> File {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap()
    }

    fn at() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    #[test]
    fn the_checksum_is_the_one_of_zip() {
        assert_eq!(crc32(0, b""), 0);
        assert_eq!(crc32(0, b"123456789"), 0xCBF4_3926);
        let whole = crc32(0, b"The quick brown fox jumps over the lazy dog");
        assert_eq!(whole, 0x414F_A339);
        let parts = crc32(
            crc32(0, b"The quick brown fox "),
            b"jumps over the lazy dog",
        );
        assert_eq!(parts, whole);
    }

    #[test]
    fn an_archive_is_read_back_with_its_names_and_its_content() {
        let directory = scratch();
        let path = directory.join("a.zip");
        let mut writer = ZipWriter::new(open(&path), open(&directory.join("d")), at());
        writer.entry("empty.txt", b"").unwrap();
        writer.start("messages/first.json").unwrap();
        writer.write(b"{\"a\":").unwrap();
        writer.write(b"1}").unwrap();
        writer.end().unwrap();
        let large: Vec<u8> = (0..200_000u32).flat_map(u32::to_le_bytes).collect();
        writer.start("attachments/grüße.bin").unwrap();
        for chunk in large.chunks(70_000) {
            writer.write(chunk).unwrap();
        }
        writer.end().unwrap();
        let expected_bytes = large.len() as u64
            + 7
            + entry_overhead("empty.txt".len())
            + entry_overhead("messages/first.json".len())
            + entry_overhead("attachments/grüße.bin".len())
            + END_BYTES;
        let bytes = writer.finish().unwrap();
        assert_eq!(bytes, expected_bytes);
        let written = std::fs::read(&path).unwrap();
        assert_eq!(written.len() as u64, bytes);
        let entries = read(&written).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["empty.txt", "messages/first.json", "attachments/grüße.bin"]
        );
        assert_eq!(entries[0].content, b"");
        assert_eq!(entries[1].content, b"{\"a\":1}");
        assert_eq!(entries[2].content, large);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_changed_byte_is_found_by_the_reader() {
        let directory = scratch();
        let path = directory.join("a.zip");
        let mut writer = ZipWriter::new(open(&path), open(&directory.join("d")), at());
        writer.entry("a.txt", b"content").unwrap();
        writer.finish().unwrap();
        let mut written = std::fs::read(&path).unwrap();
        let content_at = 30 + "a.txt".len() + 20;
        assert_eq!(&written[content_at..content_at + 7], b"content");
        written[content_at] ^= 1;
        assert!(read(&written).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_name_that_leaves_the_archive_is_refused() {
        for name in [
            "",
            "/etc/passwd",
            "../outside",
            "a/../b",
            "a/./b",
            "a//b",
            "a\\b",
            "a/",
            "line\nbreak",
        ] {
            assert!(!is_entry_name(name), "{name:?}");
        }
        for name in ["a", "a/b.txt", "attachments/0198/Report (final).pdf", "ä/ö"] {
            assert!(is_entry_name(name), "{name:?}");
        }
        let directory = scratch();
        let mut writer = ZipWriter::new(
            open(&directory.join("a.zip")),
            open(&directory.join("d")),
            at(),
        );
        assert!(writer.start("../outside").is_err());
        assert!(writer.write(b"x").is_err());
        assert!(writer.end().is_err());
        writer.start("inside").unwrap();
        assert!(writer.start("second").is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_time_of_an_entry_has_the_form_of_zip() {
        let (time, date) = dos_time(
            OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(), // 2027-01-15 08:00:00
        );
        assert_eq!(date, (47 << 9) | (1 << 5) | 15);
        assert_eq!(time, 8 << 11);
        let early = dos_time(OffsetDateTime::UNIX_EPOCH);
        assert_eq!(early.1 >> 9, 0);
    }
}
