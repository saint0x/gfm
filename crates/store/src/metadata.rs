use crate::durable;
use crate::ids::{
    read_blocked_file_id_block_from_slice, read_blocked_file_ids,
    read_blocked_file_ids_limited_from_slice, write_blocked_file_ids,
};
use crate::integrity::{verify_checksum_footer, write_checksum_footer};
use gfm_types::{FileId, FileRecord, GfmError, Result, VolumeId};
use memmap2::{Mmap, MmapOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};

const METADATA_MAGIC_V1: &[u8] = b"gfm-metadata-v1\n";
const METADATA_MAGIC_V2: &[u8] = b"gfm-metadata-v2\n";
const METADATA_MAGIC_V3: &[u8] = b"gfm-metadata-v3\n";
const METADATA_INDEX_FOOTER: &[u8] = b"gfm-metadata-index-v1\n";
const METADATA_CHECKSUM_FOOTER: &[u8] = b"gfm-metadata-checksum-v1\n";
const METADATA_FOOTER_LEN: u64 = 8 + METADATA_INDEX_FOOTER.len() as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetadataStoreVersion {
    V1,
    V2,
    V3,
}

impl MetadataStoreVersion {
    const fn uses_blocked_ids(self) -> bool {
        matches!(self, Self::V2 | Self::V3)
    }

    const fn has_checksum(self) -> bool {
        matches!(self, Self::V3)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MetadataField {
    Tag,
    Comment,
}

impl MetadataField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tag => "tag",
            Self::Comment => "comment",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "tag" => Some(Self::Tag),
            "comment" => Some(Self::Comment),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataPosting {
    pub field: MetadataField,
    pub term: String,
    pub ids: Vec<FileId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitedMetadataPosting {
    pub posting: MetadataPosting,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetadataDirectoryEntry {
    field: MetadataField,
    term: String,
    offset: u64,
    len: u64,
}

#[derive(Debug)]
pub struct MmapMetadataArchive {
    path: PathBuf,
    mmap: Mmap,
    version: MetadataStoreVersion,
    directory: Vec<MetadataDirectoryEntry>,
}

pub fn metadata_postings_from_records(records: &[FileRecord]) -> Vec<MetadataPosting> {
    let mut postings: BTreeMap<(MetadataField, String), BTreeSet<FileId>> = BTreeMap::new();
    for record in records {
        for tag in &record.tags {
            let tag = normalize(tag);
            if !tag.is_empty() {
                postings
                    .entry((MetadataField::Tag, tag))
                    .or_default()
                    .insert(record.id);
            }
        }
        if let Some(comment) = &record.finder_comment {
            for token in tokenize(&normalize(comment)) {
                postings
                    .entry((MetadataField::Comment, token))
                    .or_default()
                    .insert(record.id);
            }
        }
    }
    postings
        .into_iter()
        .map(|((field, term), ids)| MetadataPosting {
            field,
            term,
            ids: ids.into_iter().collect(),
        })
        .collect()
}

pub fn write_metadata_postings(path: impl AsRef<Path>, postings: &[MetadataPosting]) -> Result<()> {
    let path = path.as_ref();
    durable::atomic_write(path, |writer| {
        let mut bytes = Vec::new();
        {
            let mut archive = CountingWriter::new(&mut bytes);
            archive.write_all(METADATA_MAGIC_V3)?;
            write_varint(&mut archive, postings.len() as u64)?;
            let mut directory = Vec::with_capacity(postings.len());
            let mut postings = postings.to_vec();
            postings.sort_by(|left, right| {
                (left.field, left.term.as_str()).cmp(&(right.field, right.term.as_str()))
            });
            for posting in &postings {
                let offset = archive.position();
                write_metadata_posting(&mut archive, posting, MetadataStoreVersion::V3)?;
                let end = archive.position();
                directory.push(MetadataDirectoryEntry {
                    field: posting.field,
                    term: posting.term.clone(),
                    offset,
                    len: end.saturating_sub(offset),
                });
            }
            let directory_offset = archive.position();
            write_varint(&mut archive, directory.len() as u64)?;
            for entry in &directory {
                write_directory_entry(&mut archive, entry)?;
            }
            archive.write_all(&directory_offset.to_le_bytes())?;
            archive.write_all(METADATA_INDEX_FOOTER)?;
        }
        let mut footer = Vec::new();
        write_checksum_footer(&mut footer, &bytes, METADATA_CHECKSUM_FOOTER)?;
        bytes.extend(footer);
        writer.write_all(&bytes)?;
        Ok(())
    })
    .map(|_| ())
}

pub fn read_metadata_postings(path: impl AsRef<Path>) -> Result<Vec<MetadataPosting>> {
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    let mut magic = vec![0; METADATA_MAGIC_V1.len()];
    file.read_exact(&mut magic)
        .map_err(|err| GfmError::io(path, err))?;
    let version = metadata_version(&magic, path)?;
    verify_metadata_checksum_for_file(&mut file, path, version)?;
    let count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
    let mut postings = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        postings.push(read_metadata_posting(&mut file, path, version)?);
    }
    Ok(postings)
}

impl MmapMetadataArchive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mmap = {
            // SAFETY: The metadata archive is mapped read-only and accessed only
            // through bounds-checked immutable slices. Writers publish archives by
            // atomic rename, so this API never observes in-place mutation.
            unsafe { MmapOptions::new().map(&file) }.map_err(|err| GfmError::io(path, err))?
        };
        let magic = mmap
            .get(..METADATA_MAGIC_V1.len())
            .ok_or_else(|| metadata_format_error(path, "unsupported metadata header"))?;
        let version = metadata_version(magic, path)?;
        verify_metadata_checksum_from_slice(&mmap, path, version)?;
        let directory = read_metadata_directory_from_slice(&mmap, path)?;
        Ok(Self {
            path: path.to_path_buf(),
            mmap,
            version,
            directory,
        })
    }

    pub fn ids_for(&self, field: MetadataField, term: &str) -> Result<Vec<FileId>> {
        Ok(self
            .posting_for(field, term)?
            .map(|posting| posting.ids)
            .unwrap_or_default())
    }

    pub fn ids_for_limit(
        &self,
        field: MetadataField,
        term: &str,
        limit: usize,
    ) -> Result<(Vec<FileId>, bool)> {
        let term = normalize(term);
        if term.is_empty() || limit == 0 {
            return Ok((Vec::new(), false));
        }
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| {
                (entry.field, entry.term.as_str()).cmp(&(field, term.as_str()))
            })
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok((Vec::new(), false));
        };
        let posting = self.limited_posting_for_entry(entry, limit)?;
        Ok((posting.posting.ids, posting.truncated))
    }

    pub fn postings(&self) -> Result<Vec<MetadataPosting>> {
        self.directory
            .iter()
            .map(|entry| {
                let bytes = self.posting_bytes(entry)?;
                read_metadata_posting(Cursor::new(bytes), &self.path, self.version)
            })
            .collect()
    }

    pub fn postings_for<I, S>(&self, field: MetadataField, terms: I) -> Result<Vec<MetadataPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();
        for term in terms {
            let term = normalize(term.as_ref());
            if !term.is_empty() {
                selected.insert(term);
            }
        }

        selected
            .into_iter()
            .filter_map(|term| self.posting_for(field, &term).transpose())
            .collect()
    }

    pub fn postings_for_sorted_terms_limit<I, S>(
        &self,
        field: MetadataField,
        terms: I,
        limit_per_term: usize,
    ) -> Result<Vec<LimitedMetadataPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if limit_per_term == 0 {
            return Ok(Vec::new());
        }

        let mut postings = Vec::new();
        let mut directory_index = 0usize;
        let mut previous: Option<String> = None;

        for term in terms {
            let term = normalize(term.as_ref());
            if term.is_empty() {
                continue;
            }
            if let Some(previous_term) = previous.as_ref() {
                if term < *previous_term {
                    return Err(metadata_format_error(
                        &self.path,
                        "batch metadata lookup terms must be sorted",
                    ));
                }
                if term == *previous_term {
                    continue;
                }
            }

            while let Some(entry) = self.directory.get(directory_index) {
                if (entry.field, entry.term.as_str()) >= (field, term.as_str()) {
                    break;
                }
                directory_index += 1;
            }

            if let Some(entry) = self.directory.get(directory_index) {
                if entry.field == field && entry.term.as_str() == term.as_str() {
                    postings.push(self.limited_posting_for_entry(entry, limit_per_term)?);
                }
            }
            previous = Some(term);
        }

        Ok(postings)
    }

    pub fn postings_for_limit<I, S>(
        &self,
        field: MetadataField,
        terms: I,
        limit_per_term: usize,
    ) -> Result<Vec<MetadataPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();
        for term in terms {
            let term = normalize(term.as_ref());
            if !term.is_empty() {
                selected.insert(term);
            }
        }

        self.postings_for_sorted_terms_limit(field, selected, limit_per_term)
            .map(|postings| {
                postings
                    .into_iter()
                    .map(|posting| posting.posting)
                    .collect()
            })
    }

    pub fn posting_for(&self, field: MetadataField, term: &str) -> Result<Option<MetadataPosting>> {
        let term = normalize(term);
        if term.is_empty() {
            return Ok(None);
        }
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| {
                (entry.field, entry.term.as_str()).cmp(&(field, term.as_str()))
            })
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok(None);
        };
        let bytes = self.posting_bytes(entry)?;
        let posting = read_metadata_posting(Cursor::new(bytes), &self.path, self.version)?;
        if posting.field == field && posting.term == term {
            Ok(Some(posting))
        } else {
            Err(metadata_format_error(
                &self.path,
                "metadata directory points at the wrong posting",
            ))
        }
    }

    pub fn indexed_terms(&self) -> usize {
        self.directory.len()
    }

    pub fn id_block_for(
        &self,
        field: MetadataField,
        term: &str,
        block_index: usize,
    ) -> Result<Vec<FileId>> {
        let term = normalize(term);
        if term.is_empty() {
            return Ok(Vec::new());
        }
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| {
                (entry.field, entry.term.as_str()).cmp(&(field, term.as_str()))
            })
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok(Vec::new());
        };
        if !self.version.uses_blocked_ids() {
            return self.ids_for(field, &term);
        }
        let bytes = self.posting_bytes(entry)?;
        let mut cursor = Cursor::new(bytes);
        read_metadata_posting_header(&mut cursor, &self.path)?;
        let ids_start = usize::try_from(cursor.position())
            .map_err(|_| metadata_format_error(&self.path, "metadata id offset overflow"))?;
        let id_bytes = bytes
            .get(ids_start..)
            .ok_or_else(|| metadata_format_error(&self.path, "metadata id offset out of bounds"))?;
        read_blocked_file_id_block_from_slice(id_bytes, block_index, &self.path)
    }

    pub fn mapped_len(&self) -> usize {
        self.mmap.len()
    }

    pub fn is_checksummed(&self) -> bool {
        self.version.has_checksum()
    }

    fn posting_bytes(&self, entry: &MetadataDirectoryEntry) -> Result<&[u8]> {
        let start = usize::try_from(entry.offset)
            .map_err(|_| metadata_format_error(&self.path, "posting offset overflow"))?;
        let len = usize::try_from(entry.len)
            .map_err(|_| metadata_format_error(&self.path, "posting length overflow"))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| metadata_format_error(&self.path, "posting range overflow"))?;
        self.mmap
            .get(start..end)
            .ok_or_else(|| metadata_format_error(&self.path, "posting range out of bounds"))
    }

    fn limited_posting_for_entry(
        &self,
        entry: &MetadataDirectoryEntry,
        limit: usize,
    ) -> Result<LimitedMetadataPosting> {
        let bytes = self.posting_bytes(entry)?;
        let mut cursor = Cursor::new(bytes);
        let (posting_field, posting_term) = read_metadata_posting_header(&mut cursor, &self.path)?;
        if posting_field != entry.field || posting_term != entry.term {
            return Err(metadata_format_error(
                &self.path,
                "metadata directory points at the wrong posting",
            ));
        }
        let ids_start = usize::try_from(cursor.position())
            .map_err(|_| metadata_format_error(&self.path, "metadata id offset overflow"))?;
        let id_bytes = bytes
            .get(ids_start..)
            .ok_or_else(|| metadata_format_error(&self.path, "metadata id offset out of bounds"))?;
        let mut ids = match self.version {
            MetadataStoreVersion::V1 => {
                read_file_ids_limited(Cursor::new(id_bytes), &self.path, limit.saturating_add(1))?
            }
            MetadataStoreVersion::V2 | MetadataStoreVersion::V3 => {
                read_blocked_file_ids_limited_from_slice(
                    id_bytes,
                    limit.saturating_add(1),
                    &self.path,
                )?
            }
        };
        let truncated = ids.len() > limit;
        ids.truncate(limit);
        Ok(LimitedMetadataPosting {
            posting: MetadataPosting {
                field: posting_field,
                term: posting_term,
                ids,
            },
            truncated,
        })
    }
}

struct CountingWriter<'a> {
    inner: &'a mut dyn Write,
    position: u64,
}

impl<'a> CountingWriter<'a> {
    fn new(inner: &'a mut dyn Write) -> Self {
        Self { inner, position: 0 }
    }

    fn position(&self) -> u64 {
        self.position
    }
}

impl Write for CountingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.position += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn write_directory_entry(
    mut writer: impl Write,
    entry: &MetadataDirectoryEntry,
) -> std::io::Result<()> {
    writer.write_all(&[metadata_field_code(entry.field)])?;
    let term = entry.term.as_bytes();
    write_varint(&mut writer, term.len() as u64)?;
    writer.write_all(term)?;
    write_varint(&mut writer, entry.offset)?;
    write_varint(&mut writer, entry.len)
}

fn read_metadata_directory_from_slice(
    bytes: &[u8],
    path: &Path,
) -> Result<Vec<MetadataDirectoryEntry>> {
    if bytes.len() < METADATA_MAGIC_V1.len() + METADATA_FOOTER_LEN as usize {
        return Err(metadata_format_error(
            path,
            "missing metadata directory footer",
        ));
    }
    let indexed_len = metadata_indexed_len_from_slice(bytes, path)?;
    let archive_bytes = bytes
        .get(..indexed_len)
        .ok_or_else(|| metadata_format_error(path, "metadata indexed range out of bounds"))?;
    let footer_offset = archive_bytes
        .len()
        .checked_sub(METADATA_FOOTER_LEN as usize)
        .ok_or_else(|| metadata_format_error(path, "missing metadata directory footer"))?;
    let mut offset = [0u8; 8];
    offset.copy_from_slice(
        archive_bytes
            .get(footer_offset..footer_offset + 8)
            .ok_or_else(|| metadata_format_error(path, "missing metadata directory footer"))?,
    );
    let directory_offset = usize::try_from(u64::from_le_bytes(offset))
        .map_err(|_| metadata_format_error(path, "invalid metadata directory offset"))?;
    let footer = archive_bytes
        .get(footer_offset + 8..)
        .ok_or_else(|| metadata_format_error(path, "missing metadata directory footer"))?;
    if footer != METADATA_INDEX_FOOTER {
        return Err(metadata_format_error(
            path,
            "missing metadata directory footer",
        ));
    }
    if directory_offset >= footer_offset {
        return Err(metadata_format_error(
            path,
            "invalid metadata directory offset",
        ));
    }
    let mut cursor = Cursor::new(&archive_bytes[directory_offset..footer_offset]);
    let count = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
    let mut directory = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        directory.push(read_directory_entry(&mut cursor, path)?);
    }
    Ok(directory)
}

fn read_directory_entry(mut reader: impl Read, path: &Path) -> Result<MetadataDirectoryEntry> {
    let mut field = [0u8; 1];
    reader
        .read_exact(&mut field)
        .map_err(|err| GfmError::io(path, err))?;
    let field = metadata_field_from_code(field[0], path)?;
    let term_len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut term = vec![0; term_len as usize];
    reader
        .read_exact(&mut term)
        .map_err(|err| GfmError::io(path, err))?;
    let term = String::from_utf8(term).map_err(|err| {
        GfmError::Format(format!(
            "invalid UTF-8 metadata term in {}: {err}",
            path.display()
        ))
    })?;
    let offset = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    Ok(MetadataDirectoryEntry {
        field,
        term,
        offset,
        len,
    })
}

fn write_metadata_posting(
    mut writer: impl Write,
    posting: &MetadataPosting,
    version: MetadataStoreVersion,
) -> std::io::Result<()> {
    writer.write_all(&[metadata_field_code(posting.field)])?;
    let term = posting.term.as_bytes();
    write_varint(&mut writer, term.len() as u64)?;
    writer.write_all(term)?;
    match version {
        MetadataStoreVersion::V1 => write_file_ids(writer, &posting.ids),
        MetadataStoreVersion::V2 | MetadataStoreVersion::V3 => {
            write_blocked_file_ids(writer, &posting.ids)
        }
    }
}

fn read_metadata_posting(
    mut reader: impl Read,
    path: &Path,
    version: MetadataStoreVersion,
) -> Result<MetadataPosting> {
    let (field, term) = read_metadata_posting_header(&mut reader, path)?;
    let ids = match version {
        MetadataStoreVersion::V1 => read_file_ids(reader, path)?,
        MetadataStoreVersion::V2 | MetadataStoreVersion::V3 => read_blocked_file_ids(reader, path)?,
    };
    Ok(MetadataPosting { field, term, ids })
}

fn read_metadata_posting_header(
    mut reader: impl Read,
    path: &Path,
) -> Result<(MetadataField, String)> {
    let mut field = [0u8; 1];
    reader
        .read_exact(&mut field)
        .map_err(|err| GfmError::io(path, err))?;
    let field = metadata_field_from_code(field[0], path)?;
    let term_len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut term = vec![0; term_len as usize];
    reader
        .read_exact(&mut term)
        .map_err(|err| GfmError::io(path, err))?;
    let term = String::from_utf8(term).map_err(|err| {
        GfmError::Format(format!(
            "invalid UTF-8 metadata term in {}: {err}",
            path.display()
        ))
    })?;
    Ok((field, term))
}

fn write_file_ids(mut writer: impl Write, ids: &[FileId]) -> std::io::Result<()> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    write_varint(&mut writer, ids.len() as u64)?;
    let mut previous = FileId::new(VolumeId(0), 0);
    for id in ids {
        write_varint(&mut writer, id.volume.0.saturating_sub(previous.volume.0))?;
        let node_delta = if id.volume == previous.volume {
            id.node.saturating_sub(previous.node)
        } else {
            id.node
        };
        write_varint(&mut writer, node_delta)?;
        previous = id;
    }
    Ok(())
}

fn read_file_ids(mut reader: impl Read, path: &Path) -> Result<Vec<FileId>> {
    let id_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut ids = Vec::with_capacity(id_count.min(1_000_000) as usize);
    let mut previous = FileId::new(VolumeId(0), 0);
    for _ in 0..id_count {
        let volume_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let volume = previous
            .volume
            .0
            .checked_add(volume_delta)
            .ok_or_else(|| metadata_format_error(path, "volume id overflow"))?;
        let node_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let node = if volume == previous.volume.0 {
            previous
                .node
                .checked_add(node_delta)
                .ok_or_else(|| metadata_format_error(path, "file node id overflow"))?
        } else {
            node_delta
        };
        let id = FileId::new(VolumeId(volume), node);
        ids.push(id);
        previous = id;
    }
    Ok(ids)
}

fn read_file_ids_limited(mut reader: impl Read, path: &Path, limit: usize) -> Result<Vec<FileId>> {
    let id_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut ids = Vec::with_capacity((id_count as usize).min(limit));
    let mut previous = FileId::new(VolumeId(0), 0);
    for _ in 0..id_count.min(limit as u64) {
        let volume_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let volume = previous
            .volume
            .0
            .checked_add(volume_delta)
            .ok_or_else(|| metadata_format_error(path, "volume id overflow"))?;
        let node_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let node = if volume == previous.volume.0 {
            previous
                .node
                .checked_add(node_delta)
                .ok_or_else(|| metadata_format_error(path, "file node id overflow"))?
        } else {
            node_delta
        };
        let id = FileId::new(VolumeId(volume), node);
        ids.push(id);
        previous = id;
    }
    Ok(ids)
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
    let mut shift = 0u32;
    loop {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte)?;
        value |= ((byte[0] & 0x7f) as u64) << shift;
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

fn metadata_field_code(field: MetadataField) -> u8 {
    match field {
        MetadataField::Tag => b't',
        MetadataField::Comment => b'c',
    }
}

fn metadata_field_from_code(value: u8, path: &Path) -> Result<MetadataField> {
    match value {
        b't' => Ok(MetadataField::Tag),
        b'c' => Ok(MetadataField::Comment),
        _ => Err(metadata_format_error(path, "invalid metadata field")),
    }
}

fn metadata_version(bytes: &[u8], path: &Path) -> Result<MetadataStoreVersion> {
    if bytes == METADATA_MAGIC_V3 {
        Ok(MetadataStoreVersion::V3)
    } else if bytes == METADATA_MAGIC_V2 {
        Ok(MetadataStoreVersion::V2)
    } else if bytes == METADATA_MAGIC_V1 {
        Ok(MetadataStoreVersion::V1)
    } else {
        Err(metadata_format_error(path, "unsupported metadata header"))
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn metadata_format_error(path: &Path, reason: &str) -> GfmError {
    GfmError::Format(format!(
        "invalid metadata store {}: {reason}",
        path.display()
    ))
}

fn verify_metadata_checksum_for_file(
    file: &mut File,
    path: &Path,
    version: MetadataStoreVersion,
) -> Result<()> {
    if !version.has_checksum() {
        return Ok(());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|err| GfmError::io(path, err))?;
    let mut full = Vec::with_capacity(METADATA_MAGIC_V1.len() + bytes.len());
    full.extend(METADATA_MAGIC_V3);
    full.extend(bytes);
    verify_metadata_checksum_from_slice(&full, path, version)?;
    let data_start = METADATA_MAGIC_V1.len() as u64;
    file.seek(std::io::SeekFrom::Start(data_start))
        .map_err(|err| GfmError::io(path, err))?;
    Ok(())
}

fn verify_metadata_checksum_from_slice(
    bytes: &[u8],
    path: &Path,
    version: MetadataStoreVersion,
) -> Result<()> {
    if version.has_checksum()
        && !verify_checksum_footer(bytes, METADATA_CHECKSUM_FOOTER, path, "metadata")?
    {
        return Err(metadata_format_error(
            path,
            "missing metadata checksum footer",
        ));
    }
    Ok(())
}

fn metadata_indexed_len_from_slice(bytes: &[u8], path: &Path) -> Result<usize> {
    let footer_len = metadata_checksum_footer_len();
    if bytes.len() < footer_len {
        return Ok(bytes.len());
    }
    let footer_start = bytes.len() - footer_len;
    if bytes.get(footer_start + 4..) == Some(METADATA_CHECKSUM_FOOTER) {
        Ok(footer_start)
    } else {
        if bytes.starts_with(METADATA_MAGIC_V3) {
            return Err(metadata_format_error(
                path,
                "missing metadata checksum footer",
            ));
        }
        Ok(bytes.len())
    }
}

const fn metadata_checksum_footer_len() -> usize {
    4 + METADATA_CHECKSUM_FOOTER.len()
}

#[cfg(test)]
mod tests;
