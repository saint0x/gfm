use crate::durable;
use crate::integrity::{verify_checksum_footer, write_checksum_footer};
use gfm_types::{FileId, FileRecord, GfmError, Result, VolumeId};
use memmap2::{Mmap, MmapOptions};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

const COLUMNS_MAGIC_V1: &[u8] = b"gfm-record-columns-v1\n";
const COLUMNS_MAGIC_V2: &[u8] = b"gfm-record-columns-v2\n";
const COLUMNS_INDEX_FOOTER: &[u8] = b"gfm-record-columns-index-v1\n";
const COLUMNS_CHECKSUM_FOOTER: &[u8] = b"gfm-record-columns-checksum-v1\n";
const COLUMNS_FOOTER_LEN: u64 = 8 + COLUMNS_INDEX_FOOTER.len() as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnsVersion {
    V1,
    V2,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct StringPoolEntry {
    offset: u64,
    len: u64,
}

#[derive(Debug)]
pub struct MmapRecordColumns {
    path: PathBuf,
    mmap: Mmap,
    version: ColumnsVersion,
    directory: Vec<ColumnDirectoryEntry>,
    strings: Vec<StringPoolEntry>,
}

pub fn write_record_columns(path: impl AsRef<Path>, records: &[FileRecord]) -> Result<()> {
    write_record_columns_checked(path, records, || Ok(()))
}

pub fn write_record_columns_checked(
    path: impl AsRef<Path>,
    records: &[FileRecord],
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    write_record_columns_v2_checked(path, records, &mut check_control)
}

fn write_record_columns_v2_checked(
    path: impl AsRef<Path>,
    records: &[FileRecord],
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    let path = path.as_ref();
    let columns = records
        .iter()
        .map(RecordColumn::from_record)
        .collect::<Vec<_>>();
    let string_ids = string_pool_for_columns(&columns);
    durable::atomic_write_checked(path, &mut check_control, |writer, check_control| {
        let mut bytes = Vec::new();
        bytes
            .write_all(COLUMNS_MAGIC_V2)
            .map_err(|err| GfmError::io(path, err))?;
        write_varint(&mut bytes, records.len() as u64).map_err(|err| GfmError::io(path, err))?;
        write_varint(&mut bytes, string_ids.len() as u64).map_err(|err| GfmError::io(path, err))?;
        for value in string_ids.keys() {
            check_control()?;
            write_string(&mut bytes, value).map_err(|err| GfmError::io(path, err))?;
        }

        let mut directory = Vec::with_capacity(columns.len());
        for column in &columns {
            check_control()?;
            let offset = bytes.len() as u64;
            write_column_v2(&mut bytes, column, &string_ids)
                .map_err(|err| GfmError::io(path, err))?;
            directory.push(ColumnDirectoryEntry {
                id: column.id,
                offset,
                len: bytes.len() as u64 - offset,
            });
        }
        let directory_offset = bytes.len() as u64;
        write_varint(&mut bytes, directory.len() as u64).map_err(|err| GfmError::io(path, err))?;
        for entry in &directory {
            check_control()?;
            write_varint(&mut bytes, entry.id.volume.0).map_err(|err| GfmError::io(path, err))?;
            write_varint(&mut bytes, entry.id.node).map_err(|err| GfmError::io(path, err))?;
            write_varint(&mut bytes, entry.offset).map_err(|err| GfmError::io(path, err))?;
            write_varint(&mut bytes, entry.len).map_err(|err| GfmError::io(path, err))?;
        }
        check_control()?;
        bytes
            .write_all(&directory_offset.to_le_bytes())
            .map_err(|err| GfmError::io(path, err))?;
        bytes
            .write_all(COLUMNS_INDEX_FOOTER)
            .map_err(|err| GfmError::io(path, err))?;
        let mut footer = Vec::new();
        write_checksum_footer(&mut footer, &bytes, COLUMNS_CHECKSUM_FOOTER)
            .map_err(|err| GfmError::io(path, err))?;
        bytes.extend(footer);
        check_control()?;
        writer
            .write_all(&bytes)
            .map_err(|err| GfmError::io(path, err))?;
        Ok(())
    })
    .map(|_| ())
}

#[cfg(test)]
pub(crate) fn write_record_columns_v1(
    path: impl AsRef<Path>,
    records: &[FileRecord],
) -> Result<()> {
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
        Self::open_checked(path, || Ok(()))
    }

    pub fn open_checked(
        path: impl AsRef<Path>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let path = path.as_ref();
        check_control()?;
        let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        check_control()?;
        let mmap = {
            // SAFETY: The map is read-only and all reads are bounds checked.
            unsafe { MmapOptions::new().map(&file) }.map_err(|err| GfmError::io(path, err))?
        };
        check_control()?;
        let version = columns_version_from_slice(&mmap, path)?;
        check_control()?;
        verify_columns_checksum_from_slice(&mmap, path)?;
        check_control()?;
        let strings = match version {
            ColumnsVersion::V1 => Vec::new(),
            ColumnsVersion::V2 => read_string_pool_directory_from_slice(&mmap, path)?,
        };
        check_control()?;
        let directory = read_columns_directory_from_slice(&mmap, path)?;
        check_control()?;
        Ok(Self {
            path: path.to_path_buf(),
            mmap,
            version,
            directory,
            strings,
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

    pub fn string_pool_len(&self) -> usize {
        self.strings.len()
    }

    pub fn column(&self, index: usize) -> Result<RecordColumn> {
        self.column_checked(index, || Ok(()))
    }

    pub fn column_checked(
        &self,
        index: usize,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<RecordColumn> {
        check_control()?;
        let entry = self.directory.get(index).ok_or_else(|| {
            GfmError::Format(format!(
                "{} record column index {index} out of bounds",
                self.path.display()
            ))
        })?;
        check_control()?;
        self.read_entry(entry)
    }

    pub fn find(&self, id: FileId) -> Result<Option<RecordColumn>> {
        self.find_checked(id, || Ok(()))
    }

    pub fn find_checked(
        &self,
        id: FileId,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Option<RecordColumn>> {
        check_control()?;
        match self.directory.binary_search_by_key(&id, |entry| entry.id) {
            Ok(index) => self.column_checked(index, check_control).map(Some),
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
        let column = match self.version {
            ColumnsVersion::V1 => read_column(Cursor::new(bytes), &self.path)?,
            ColumnsVersion::V2 => read_column_v2(Cursor::new(bytes), self)?,
        };
        if column.id != entry.id {
            return Err(column_format_error(
                &self.path,
                "record column directory points at the wrong id",
            ));
        }
        Ok(column)
    }

    fn string_by_id(&self, id: u64) -> Result<String> {
        let index = usize::try_from(id)
            .map_err(|_| column_format_error(&self.path, "string pool id overflow"))?;
        let entry = self
            .strings
            .get(index)
            .ok_or_else(|| column_format_error(&self.path, "string pool id out of bounds"))?;
        let start = usize::try_from(entry.offset)
            .map_err(|_| column_format_error(&self.path, "string pool offset overflow"))?;
        let len = usize::try_from(entry.len)
            .map_err(|_| column_format_error(&self.path, "string pool length overflow"))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| column_format_error(&self.path, "string pool range overflow"))?;
        let bytes = self
            .mmap
            .get(start..end)
            .ok_or_else(|| column_format_error(&self.path, "string pool range out of bounds"))?;
        String::from_utf8(bytes.to_vec()).map_err(|err| {
            GfmError::Format(format!(
                "invalid record columns {}: pooled string is not UTF-8: {err}",
                self.path.display()
            ))
        })
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

#[cfg(test)]
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

fn write_column_v2(
    mut writer: impl Write,
    column: &RecordColumn,
    string_ids: &BTreeMap<String, u64>,
) -> std::io::Result<()> {
    write_varint(&mut writer, column.id.volume.0)?;
    write_varint(&mut writer, column.id.node)?;
    write_string_id(&mut writer, string_ids, &column.name)?;
    write_string_id(&mut writer, string_ids, &column.path)?;
    write_optional_string_id(&mut writer, string_ids, column.extension.as_deref())?;
    write_varint(&mut writer, column.tags.len() as u64)?;
    for tag in &column.tags {
        write_string_id(&mut writer, string_ids, tag)?;
    }
    write_optional_string_id(&mut writer, string_ids, column.comment.as_deref())
}

fn write_string_id(
    mut writer: impl Write,
    string_ids: &BTreeMap<String, u64>,
    value: &str,
) -> std::io::Result<()> {
    let id = string_ids
        .get(value)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing string id"))?;
    write_varint(&mut writer, *id)
}

fn write_optional_string_id(
    mut writer: impl Write,
    string_ids: &BTreeMap<String, u64>,
    value: Option<&str>,
) -> std::io::Result<()> {
    match value {
        Some(value) => {
            writer.write_all(&[1])?;
            write_string_id(writer, string_ids, value)
        }
        None => writer.write_all(&[0]),
    }
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

fn read_column_v2(mut reader: impl Read, archive: &MmapRecordColumns) -> Result<RecordColumn> {
    let volume = read_varint(&mut reader).map_err(|err| GfmError::io(&archive.path, err))?;
    let node = read_varint(&mut reader).map_err(|err| GfmError::io(&archive.path, err))?;
    let name = archive
        .string_by_id(read_varint(&mut reader).map_err(|err| GfmError::io(&archive.path, err))?)?;
    let column_path = archive
        .string_by_id(read_varint(&mut reader).map_err(|err| GfmError::io(&archive.path, err))?)?;
    let extension = read_optional_string_id(&mut reader, archive)?;
    let tag_count = read_varint(&mut reader).map_err(|err| GfmError::io(&archive.path, err))?;
    let mut tags = Vec::with_capacity(tag_count.min(1_000_000) as usize);
    for _ in 0..tag_count {
        tags.push(archive.string_by_id(
            read_varint(&mut reader).map_err(|err| GfmError::io(&archive.path, err))?,
        )?);
    }
    let comment = read_optional_string_id(&mut reader, archive)?;
    Ok(RecordColumn {
        id: FileId::new(VolumeId(volume), node),
        name,
        path: column_path,
        extension,
        tags,
        comment,
    })
}

fn read_optional_string_id(
    mut reader: impl Read,
    archive: &MmapRecordColumns,
) -> Result<Option<String>> {
    let mut present = [0u8; 1];
    reader
        .read_exact(&mut present)
        .map_err(|err| GfmError::io(&archive.path, err))?;
    match present[0] {
        0 => Ok(None),
        1 => archive
            .string_by_id(read_varint(reader).map_err(|err| GfmError::io(&archive.path, err))?)
            .map(Some),
        _ => Err(column_format_error(
            &archive.path,
            "invalid optional string id tag",
        )),
    }
}

fn string_pool_for_columns(columns: &[RecordColumn]) -> BTreeMap<String, u64> {
    let mut strings = BTreeMap::new();
    for column in columns {
        strings.insert(column.name.clone(), 0);
        strings.insert(column.path.clone(), 0);
        if let Some(extension) = &column.extension {
            strings.insert(extension.clone(), 0);
        }
        for tag in &column.tags {
            strings.insert(tag.clone(), 0);
        }
        if let Some(comment) = &column.comment {
            strings.insert(comment.clone(), 0);
        }
    }
    for (index, id) in strings.values_mut().enumerate() {
        *id = index as u64;
    }
    strings
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

#[cfg(test)]
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

fn read_string_pool_directory_from_slice(
    bytes: &[u8],
    path: &Path,
) -> Result<Vec<StringPoolEntry>> {
    let indexed_len = columns_indexed_len_from_slice(bytes, path)?;
    let directory_offset = columns_directory_offset(bytes)
        .ok_or_else(|| column_format_error(path, "missing record columns directory footer"))?;
    if directory_offset > indexed_len {
        return Err(column_format_error(
            path,
            "record columns directory offset out of bounds",
        ));
    }
    let mut cursor = Cursor::new(
        bytes
            .get(COLUMNS_MAGIC_V2.len()..directory_offset)
            .ok_or_else(|| column_format_error(path, "record columns string pool out of bounds"))?,
    );
    let _record_count = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
    let string_count = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
    let mut entries = Vec::with_capacity(string_count.min(1_000_000) as usize);
    for _ in 0..string_count {
        let len = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
        let start = COLUMNS_MAGIC_V2
            .len()
            .checked_add(usize::try_from(cursor.position()).map_err(|_| {
                column_format_error(path, "record columns string pool offset overflow")
            })?)
            .ok_or_else(|| {
                column_format_error(path, "record columns string pool offset overflow")
            })?;
        let len_usize = usize::try_from(len)
            .map_err(|_| column_format_error(path, "record columns string length overflow"))?;
        let end = start
            .checked_add(len_usize)
            .ok_or_else(|| column_format_error(path, "record columns string range overflow"))?;
        if end > directory_offset {
            return Err(column_format_error(
                path,
                "record columns string crosses column data",
            ));
        }
        cursor.set_position(
            cursor.position().checked_add(len).ok_or_else(|| {
                column_format_error(path, "record columns string offset overflow")
            })?,
        );
        entries.push(StringPoolEntry {
            offset: start as u64,
            len,
        });
    }
    Ok(entries)
}

fn columns_version_from_slice(bytes: &[u8], path: &Path) -> Result<ColumnsVersion> {
    if bytes.starts_with(COLUMNS_MAGIC_V2) {
        Ok(ColumnsVersion::V2)
    } else if bytes.starts_with(COLUMNS_MAGIC_V1) {
        Ok(ColumnsVersion::V1)
    } else {
        Err(column_format_error(
            path,
            "unsupported record columns header",
        ))
    }
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
        let legacy_path = temp_path("gfm-record-columns-legacy", "gfmcols");
        let records = vec![
            record(2, "/tmp/Archive.PDF", "Archive.PDF", &["Later"], None),
            record(
                1,
                "/tmp/Important.md",
                "Important.md",
                &["Important", "Later"],
                Some("Launch Notes"),
            ),
            record(
                3,
                "/tmp/Team/Launch Notes 3.md",
                "Launch Notes 3.md",
                &["Important", "Later"],
                Some("Launch Notes"),
            ),
            record(
                4,
                "/tmp/Team/Launch Notes 4.md",
                "Launch Notes 4.md",
                &["Important", "Later"],
                Some("Launch Notes"),
            ),
            record(
                5,
                "/tmp/Team/Launch Notes 5.md",
                "Launch Notes 5.md",
                &["Important", "Later"],
                Some("Launch Notes"),
            ),
            record(
                6,
                "/tmp/Team/Launch Notes 6.md",
                "Launch Notes 6.md",
                &["Important", "Later"],
                Some("Launch Notes"),
            ),
        ];

        write_record_columns(&path, &records).unwrap();
        write_record_columns_v1(&legacy_path, &records).unwrap();
        let archive = MmapRecordColumns::open(&path).unwrap();
        let legacy = MmapRecordColumns::open(&legacy_path).unwrap();
        let first = archive.find(records[1].id).unwrap().unwrap();

        assert_eq!(archive.len(), records.len());
        assert!(archive.is_checksummed());
        assert!(archive.string_pool_len() >= 7);
        assert!(
            std::fs::metadata(&path).unwrap().len()
                < std::fs::metadata(&legacy_path).unwrap().len()
        );
        assert_eq!(archive.column(0).unwrap().id, records[1].id);
        assert_eq!(first.name, "important.md");
        assert_eq!(first.path, "/tmp/important.md");
        assert_eq!(first.extension.as_deref(), Some("md"));
        assert_eq!(first.tags, vec!["important", "later"]);
        assert_eq!(first.comment.as_deref(), Some("launch notes"));
        assert_eq!(legacy.find(records[1].id).unwrap().unwrap(), first);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(legacy_path).unwrap();
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

    #[test]
    fn mmap_record_columns_checked_open_honors_pre_cancelled_control_before_file_open() {
        let path = temp_path("gfm-record-columns-open-cancel", "gfmcols");

        let result = MmapRecordColumns::open_checked(&path, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!path.exists());
    }

    #[test]
    fn mmap_record_columns_checked_column_honors_pre_cancelled_control() {
        let path = temp_path("gfm-record-columns-column-cancel", "gfmcols");
        let records = vec![record(1, "/tmp/Important.md", "Important.md", &[], None)];
        write_record_columns(&path, &records).unwrap();
        let archive = MmapRecordColumns::open(&path).unwrap();

        let result = archive.column_checked(0, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_record_columns_checked_find_honors_pre_cancelled_control() {
        let path = temp_path("gfm-record-columns-find-cancel", "gfmcols");
        let records = vec![record(1, "/tmp/Important.md", "Important.md", &[], None)];
        write_record_columns(&path, &records).unwrap();
        let archive = MmapRecordColumns::open(&path).unwrap();

        let result = archive.find_checked(records[0].id, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
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
