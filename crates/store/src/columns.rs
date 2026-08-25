use crate::durable;
use crate::integrity::{verify_checksum_footer, write_checksum_footer};
use gfm_types::{FileId, FileRecord, GfmError, Result, VolumeId};
use memmap2::{Mmap, MmapOptions};
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

const COLUMNS_MAGIC_V1: &[u8] = b"gfm-record-columns-v1\n";
const COLUMNS_INDEX_FOOTER: &[u8] = b"gfm-record-columns-index-v1\n";
const COLUMNS_CHECKSUM_FOOTER: &[u8] = b"gfm-record-columns-checksum-v1\n";
const COLUMNS_FOOTER_LEN: u64 = 8 + COLUMNS_INDEX_FOOTER.len() as u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordColumn {
    pub id: FileId,
    pub name: String,
    pub path: String,
    pub extension: Option<String>,
    pub tags: Vec<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ColumnDirectoryEntry {
    id: FileId,
    offset: u64,
    len: u64,
}

#[derive(Debug)]
pub struct MmapRecordColumns {
    path: PathBuf,
    mmap: Mmap,
    directory: Vec<ColumnDirectoryEntry>,
}

pub fn write_record_columns(path: impl AsRef<Path>, records: &[FileRecord]) -> Result<()> {
    let path = path.as_ref();
    durable::atomic_write(path, |writer| {
        let mut bytes = Vec::new();
        bytes.write_all(COLUMNS_MAGIC_V1)?;
        write_varint(&mut bytes, records.len() as u64)?;
        let mut directory = Vec::with_capacity(records.len());
        for record in records {
            let offset = bytes.len() as u64;
            write_column(&mut bytes, &RecordColumn::from_record(record))?;
            directory.push(ColumnDirectoryEntry {
                id: record.id,
                offset,
                len: bytes.len() as u64 - offset,
            });
        }
        let directory_offset = bytes.len() as u64;
        write_varint(&mut bytes, directory.len() as u64)?;
        for entry in &directory {
            write_varint(&mut bytes, entry.id.volume.0)?;
            write_varint(&mut bytes, entry.id.node)?;
            write_varint(&mut bytes, entry.offset)?;
            write_varint(&mut bytes, entry.len)?;
        }
        bytes.write_all(&directory_offset.to_le_bytes())?;
        bytes.write_all(COLUMNS_INDEX_FOOTER)?;
        let mut footer = Vec::new();
        write_checksum_footer(&mut footer, &bytes, COLUMNS_CHECKSUM_FOOTER)?;
        bytes.extend(footer);
        writer.write_all(&bytes)?;
        Ok(())
    })
    .map(|_| ())
}

impl MmapRecordColumns {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mmap = {
            // SAFETY: The map is read-only and all reads are bounds checked.
            unsafe { MmapOptions::new().map(&file) }.map_err(|err| GfmError::io(path, err))?
        };
        if !mmap.starts_with(COLUMNS_MAGIC_V1) {
            return Err(column_format_error(
                path,
                "unsupported record columns header",
            ));
        }
        verify_columns_checksum_from_slice(&mmap, path)?;
        let directory = read_columns_directory_from_slice(&mmap, path)?;
        Ok(Self {
            path: path.to_path_buf(),
            mmap,
            directory,
        })
    }

    pub fn len(&self) -> usize {
        self.directory.len()
    }

    pub fn is_empty(&self) -> bool {
        self.directory.is_empty()
    }

    pub fn mapped_len(&self) -> usize {
        self.mmap.len()
    }

    pub fn is_checksummed(&self) -> bool {
        has_columns_checksum_footer(&self.mmap)
    }

    pub fn column(&self, index: usize) -> Result<RecordColumn> {
        let entry = self.directory.get(index).ok_or_else(|| {
            GfmError::Format(format!(
                "{} record column index {index} out of bounds",
                self.path.display()
            ))
        })?;
        self.read_entry(entry)
    }

    pub fn find(&self, id: FileId) -> Result<Option<RecordColumn>> {
        match self.directory.binary_search_by_key(&id, |entry| entry.id) {
            Ok(index) => self.column(index).map(Some),
            Err(_) => Ok(None),
        }
    }

    fn read_entry(&self, entry: &ColumnDirectoryEntry) -> Result<RecordColumn> {
        let start = usize::try_from(entry.offset)
            .map_err(|_| column_format_error(&self.path, "column offset overflow"))?;
        let indexed_len = columns_indexed_len_from_slice(&self.mmap, &self.path)?;
        let len = usize::try_from(entry.len)
            .map_err(|_| column_format_error(&self.path, "column length overflow"))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| column_format_error(&self.path, "column range overflow"))?;
        if end > columns_directory_offset(&self.mmap).unwrap_or(indexed_len) {
            return Err(column_format_error(
                &self.path,
                "record column range crosses directory",
            ));
        }
        let bytes = self
            .mmap
            .get(start..end)
            .ok_or_else(|| column_format_error(&self.path, "record column range out of bounds"))?;
        let column = read_column(Cursor::new(bytes), &self.path)?;
        if column.id != entry.id {
            return Err(column_format_error(
                &self.path,
                "record column directory points at the wrong id",
            ));
        }
        Ok(column)
    }
}

impl RecordColumn {
    fn from_record(record: &FileRecord) -> Self {
        let mut tags = record
            .tags
            .iter()
            .map(|tag| normalize(tag))
            .filter(|tag| !tag.is_empty())
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();
        Self {
            id: record.id,
            name: normalize(&record.name),
            path: normalize(&record.path.to_string_lossy()),
            extension: record.extension().map(normalize),
            tags,
            comment: record.finder_comment.as_deref().map(normalize),
        }
    }
}

fn write_column(mut writer: impl Write, column: &RecordColumn) -> std::io::Result<()> {
    write_varint(&mut writer, column.id.volume.0)?;
    write_varint(&mut writer, column.id.node)?;
    write_string(&mut writer, &column.name)?;
    write_string(&mut writer, &column.path)?;
    write_optional_string(&mut writer, column.extension.as_deref())?;
    write_varint(&mut writer, column.tags.len() as u64)?;
    for tag in &column.tags {
        write_string(&mut writer, tag)?;
    }
    write_optional_string(&mut writer, column.comment.as_deref())
}

fn read_column(mut reader: impl Read, path: &Path) -> Result<RecordColumn> {
    let volume = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let node = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let name = read_string(&mut reader, path)?;
    let column_path = read_string(&mut reader, path)?;
    let extension = read_optional_string(&mut reader, path)?;
    let tag_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut tags = Vec::with_capacity(tag_count.min(1_000_000) as usize);
    for _ in 0..tag_count {
        tags.push(read_string(&mut reader, path)?);
    }
    let comment = read_optional_string(&mut reader, path)?;
    Ok(RecordColumn {
        id: FileId::new(VolumeId(volume), node),
        name,
        path: column_path,
        extension,
        tags,
        comment,
    })
}

fn write_string(mut writer: impl Write, value: &str) -> std::io::Result<()> {
    write_varint(&mut writer, value.len() as u64)?;
    writer.write_all(value.as_bytes())
}

fn read_string(mut reader: impl Read, path: &Path) -> Result<String> {
    let len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let len = usize::try_from(len).map_err(|_| column_format_error(path, "string overflow"))?;
    let mut bytes = vec![0; len];
    reader
        .read_exact(&mut bytes)
        .map_err(|err| GfmError::io(path, err))?;
    String::from_utf8(bytes).map_err(|err| {
        GfmError::Format(format!(
            "invalid record columns {}: string is not UTF-8: {err}",
            path.display()
        ))
    })
}

fn write_optional_string(mut writer: impl Write, value: Option<&str>) -> std::io::Result<()> {
    match value {
        Some(value) => {
            writer.write_all(&[1])?;
            write_string(writer, value)
        }
        None => writer.write_all(&[0]),
    }
}

fn read_optional_string(mut reader: impl Read, path: &Path) -> Result<Option<String>> {
    let mut present = [0u8; 1];
    reader
        .read_exact(&mut present)
        .map_err(|err| GfmError::io(path, err))?;
    match present[0] {
        0 => Ok(None),
        1 => read_string(reader, path).map(Some),
        _ => Err(column_format_error(path, "invalid optional string tag")),
    }
}

fn read_columns_directory_from_slice(
    bytes: &[u8],
    path: &Path,
) -> Result<Vec<ColumnDirectoryEntry>> {
    let indexed_len = columns_indexed_len_from_slice(bytes, path)?;
    let archive = bytes
        .get(..indexed_len)
        .ok_or_else(|| column_format_error(path, "record columns indexed range out of bounds"))?;
    let directory_offset = columns_directory_offset(bytes)
        .ok_or_else(|| column_format_error(path, "missing record columns directory footer"))?;
    let footer_offset = archive
        .len()
        .checked_sub(COLUMNS_FOOTER_LEN as usize)
        .ok_or_else(|| column_format_error(path, "missing record columns directory footer"))?;
    if directory_offset < COLUMNS_MAGIC_V1.len() || directory_offset >= footer_offset {
        return Err(column_format_error(
            path,
            "invalid record columns directory offset",
        ));
    }
    let footer = archive
        .get(footer_offset + 8..)
        .ok_or_else(|| column_format_error(path, "missing record columns directory footer"))?;
    if footer != COLUMNS_INDEX_FOOTER {
        return Err(column_format_error(
            path,
            "missing record columns directory footer",
        ));
    }
    let mut cursor = Cursor::new(&archive[directory_offset..footer_offset]);
    let count = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
    let mut entries = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        let volume = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
        let node = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
        let offset = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
        let len = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
        entries.push(ColumnDirectoryEntry {
            id: FileId::new(VolumeId(volume), node),
            offset,
            len,
        });
    }
    entries.sort_by_key(|entry| entry.id);
    entries.dedup_by_key(|entry| entry.id);
    Ok(entries)
}

fn columns_directory_offset(bytes: &[u8]) -> Option<usize> {
    let indexed_len = columns_indexed_len_from_slice(bytes, Path::new("<record-columns>")).ok()?;
    if indexed_len < COLUMNS_FOOTER_LEN as usize {
        return None;
    }
    let footer_offset = indexed_len - COLUMNS_FOOTER_LEN as usize;
    let mut offset = [0u8; 8];
    offset.copy_from_slice(bytes.get(footer_offset..footer_offset + 8)?);
    usize::try_from(u64::from_le_bytes(offset)).ok()
}

fn verify_columns_checksum_from_slice(bytes: &[u8], path: &Path) -> Result<()> {
    if has_columns_checksum_footer(bytes)
        && !verify_checksum_footer(bytes, COLUMNS_CHECKSUM_FOOTER, path, "record columns")?
    {
        return Err(column_format_error(
            path,
            "missing record columns checksum footer",
        ));
    }
    Ok(())
}

fn columns_indexed_len_from_slice(bytes: &[u8], _path: &Path) -> Result<usize> {
    let footer_len = columns_checksum_footer_len();
    if bytes.len() >= footer_len
        && bytes.get(bytes.len() - footer_len + 4..) == Some(COLUMNS_CHECKSUM_FOOTER)
    {
        Ok(bytes.len() - footer_len)
    } else {
        Ok(bytes.len())
    }
}

fn has_columns_checksum_footer(bytes: &[u8]) -> bool {
    let footer_len = columns_checksum_footer_len();
    bytes.len() >= footer_len
        && bytes.get(bytes.len() - footer_len + 4..) == Some(COLUMNS_CHECKSUM_FOOTER)
}

const fn columns_checksum_footer_len() -> usize {
    4 + COLUMNS_CHECKSUM_FOOTER.len()
}

fn column_format_error(path: &Path, reason: &str) -> GfmError {
    GfmError::Format(format!(
        "invalid record columns {}: {reason}",
        path.display()
    ))
}

fn normalize(input: &str) -> String {
    input.trim().to_lowercase()
}

fn write_varint(mut writer: impl Write, mut value: u64) -> std::io::Result<()> {
    while value >= 0x80 {
        writer.write_all(&[((value as u8) & 0x7f) | 0x80])?;
        value >>= 7;
    }
    writer.write_all(&[value as u8])
}

fn read_varint(mut reader: impl Read) -> std::io::Result<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte)?;
        value |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= 64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "varint overflow",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::{FileKind, FileRecord};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn mmap_record_columns_read_normalized_random_access_columns() {
        let path = temp_path("gfm-record-columns", "gfmcols");
        let records = vec![
            record(2, "/tmp/Archive.PDF", "Archive.PDF", &["Later"], None),
            record(
                1,
                "/tmp/Important.md",
                "Important.md",
                &["Important", "Later"],
                Some("Launch Notes"),
            ),
        ];

        write_record_columns(&path, &records).unwrap();
        let archive = MmapRecordColumns::open(&path).unwrap();
        let first = archive.find(records[1].id).unwrap().unwrap();

        assert_eq!(archive.len(), 2);
        assert!(archive.is_checksummed());
        assert_eq!(archive.column(0).unwrap().id, records[1].id);
        assert_eq!(first.name, "important.md");
        assert_eq!(first.path, "/tmp/important.md");
        assert_eq!(first.extension.as_deref(), Some("md"));
        assert_eq!(first.tags, vec!["important", "later"]);
        assert_eq!(first.comment.as_deref(), Some("launch notes"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn checksummed_record_columns_reject_corruption() {
        let path = temp_path("gfm-record-columns-checksum", "gfmcols");
        let records = vec![record(1, "/tmp/Important.md", "Important.md", &[], None)];

        write_record_columns(&path, &records).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let offset = bytes
            .windows(b"important".len())
            .position(|window| window == b"important")
            .expect("archive should contain normalized test name");
        bytes[offset] = b'z';
        std::fs::write(&path, bytes).unwrap();

        let error = MmapRecordColumns::open(&path).unwrap_err().to_string();

        assert!(error.contains("checksum mismatch"), "{error}");
        std::fs::remove_file(path).unwrap();
    }

    fn record(
        node: u64,
        path: &str,
        name: &str,
        tags: &[&str],
        comment: Option<&str>,
    ) -> FileRecord {
        FileRecord {
            id: FileId::new(VolumeId(4), node),
            parent: Some(FileId::new(VolumeId(4), 1)),
            path: PathBuf::from(path),
            name: name.to_string(),
            kind: FileKind::File,
            len: 42,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: Some(UNIX_EPOCH + Duration::from_secs(1)),
            modified: Some(UNIX_EPOCH + Duration::from_secs(10)),
            changed: None,
            hidden: false,
            tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
            finder_comment: comment.map(ToOwned::to_owned),
        }
    }

    fn temp_path(prefix: &str, extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{}-{}.{}",
            prefix,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            extension
        ))
    }
}
