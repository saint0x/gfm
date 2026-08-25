use crate::durable;
use crate::ids::{
    read_blocked_file_id_block_from_slice, read_blocked_file_ids, write_blocked_file_ids,
};
use crate::integrity::{verify_checksum_footer, write_checksum_footer};
use gfm_types::{
    ContentPositions, ContentPosting, ContentSegment, FileId, GfmError, Result, VolumeId,
};
use memmap2::{Mmap, MmapOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const CONTENT_MAGIC_V1: &[u8] = b"gfm-content-v1\n";
const CONTENT_MAGIC_V2: &[u8] = b"gfm-content-v2\n";
const CONTENT_MAGIC_V3: &[u8] = b"gfm-content-v3\n";
const CONTENT_MAGIC_V4: &[u8] = b"gfm-content-v4\n";
const CONTENT_MAGIC_V5: &[u8] = b"gfm-content-v5\n";
const CONTENT_SEGMENT_MAGIC: &[u8] = b"gfm-content-segment-v1\n";
const CONTENT_SEGMENT_MAGIC_V2: &[u8] = b"gfm-content-segment-v2\n";
const CONTENT_INDEX_FOOTER: &[u8] = b"gfm-content-index-v1\n";
const CONTENT_CHECKSUM_FOOTER: &[u8] = b"gfm-content-checksum-v1\n";
const CONTENT_FOOTER_LEN: u64 = 8 + CONTENT_INDEX_FOOTER.len() as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentStoreVersion {
    Legacy,
    IndexedIds,
    IndexedPositions,
    IndexedBlockedPositions,
    IndexedChecksummed,
}

impl ContentStoreVersion {
    const fn uses_positions(self) -> bool {
        matches!(
            self,
            Self::IndexedPositions | Self::IndexedBlockedPositions | Self::IndexedChecksummed
        )
    }

    const fn uses_blocked_ids(self) -> bool {
        matches!(
            self,
            Self::IndexedBlockedPositions | Self::IndexedChecksummed
        )
    }

    const fn has_checksum(self) -> bool {
        matches!(self, Self::IndexedChecksummed)
    }
}

pub fn write_content_postings(path: impl AsRef<Path>, postings: &[ContentPosting]) -> Result<()> {
    let path = path.as_ref();
    durable::atomic_write(path, |writer| {
        let mut bytes = Vec::new();
        {
            let mut archive = CountingWriter::new(&mut bytes);
            archive.write_all(CONTENT_MAGIC_V5)?;
            write_varint(&mut archive, postings.len() as u64)?;
            let mut directory = Vec::with_capacity(postings.len());
            for posting in postings {
                let offset = archive.position();
                write_content_posting(
                    &mut archive,
                    posting,
                    ContentStoreVersion::IndexedChecksummed,
                )?;
                let end = archive.position();
                directory.push(ContentDirectoryEntry {
                    term: posting.term.trim().to_lowercase(),
                    offset,
                    len: end.saturating_sub(offset),
                });
            }
            directory.sort_by(|left, right| left.term.cmp(&right.term));

            let directory_offset = archive.position();
            write_varint(&mut archive, directory.len() as u64)?;
            for entry in &directory {
                write_directory_entry(&mut archive, entry)?;
            }
            archive.write_all(&directory_offset.to_le_bytes())?;
            archive.write_all(CONTENT_INDEX_FOOTER)?;
        }
        let mut footer = Vec::new();
        write_checksum_footer(&mut footer, &bytes, CONTENT_CHECKSUM_FOOTER)?;
        bytes.extend(footer);
        writer.write_all(&bytes)?;
        Ok(())
    })
    .map(|_| ())
}

pub fn read_content_postings(path: impl AsRef<Path>) -> Result<Vec<ContentPosting>> {
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    let magic = read_content_magic(&mut file, path)?;
    let version = content_version(&magic, path)?;
    if version == ContentStoreVersion::Legacy && magic != CONTENT_MAGIC_V1 {
        return Err(GfmError::Format(format!(
            "unsupported content store header in {}",
            path.display()
        )));
    }
    verify_content_checksum_for_file(&mut file, path, version)?;

    let count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
    let mut postings = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        postings.push(read_content_posting(&mut file, path, version)?);
    }
    Ok(postings)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContentDirectoryEntry {
    term: String,
    offset: u64,
    len: u64,
}

#[derive(Debug)]
enum ContentArchiveDirectory {
    Indexed(Vec<ContentDirectoryEntry>),
    Legacy(Vec<ContentPosting>),
}

#[derive(Debug)]
pub struct ContentArchive {
    path: PathBuf,
    file: File,
    directory: ContentArchiveDirectory,
    version: ContentStoreVersion,
}

#[derive(Debug)]
pub struct MmapContentArchive {
    path: PathBuf,
    mmap: Mmap,
    directory: Vec<ContentDirectoryEntry>,
    version: ContentStoreVersion,
}

impl ContentArchive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        let magic = read_content_magic(&mut file, path)?;
        let version = content_version(&magic, path)?;
        if matches!(
            version,
            ContentStoreVersion::IndexedIds
                | ContentStoreVersion::IndexedPositions
                | ContentStoreVersion::IndexedBlockedPositions
                | ContentStoreVersion::IndexedChecksummed
        ) {
            verify_content_checksum_for_file(&mut file, path, version)?;
            let directory = read_content_directory(&mut file, path)?;
            Ok(Self {
                path: path.to_path_buf(),
                file,
                directory: ContentArchiveDirectory::Indexed(directory),
                version,
            })
        } else if version == ContentStoreVersion::Legacy {
            file.seek(SeekFrom::Start(CONTENT_MAGIC_V1.len() as u64))
                .map_err(|err| GfmError::io(path, err))?;
            let count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
            let mut postings = Vec::with_capacity(count.min(1_000_000) as usize);
            for _ in 0..count {
                postings.push(read_content_posting(&mut file, path, version)?);
            }
            Ok(Self {
                path: path.to_path_buf(),
                file,
                directory: ContentArchiveDirectory::Legacy(postings),
                version,
            })
        } else {
            Err(GfmError::Format(format!(
                "unsupported content store header in {}",
                path.display()
            )))
        }
    }

    pub fn ids_for_term(&mut self, term: &str) -> Result<Vec<FileId>> {
        let term = term.trim().to_lowercase();
        if term.is_empty() {
            return Ok(Vec::new());
        }
        match &self.directory {
            ContentArchiveDirectory::Indexed(directory) => {
                let Some(entry) = directory
                    .binary_search_by(|entry| entry.term.as_str().cmp(term.as_str()))
                    .ok()
                    .map(|index| directory[index].clone())
                else {
                    return Ok(Vec::new());
                };
                self.file
                    .seek(SeekFrom::Start(entry.offset))
                    .map_err(|err| GfmError::io(&self.path, err))?;
                let posting = read_content_posting(
                    (&mut self.file).take(entry.len),
                    &self.path,
                    self.version,
                )?;
                if posting.term.trim().to_lowercase() == term {
                    Ok(posting.ids)
                } else {
                    Err(content_format_error(
                        &self.path,
                        "content directory points at the wrong term",
                    ))
                }
            }
            ContentArchiveDirectory::Legacy(postings) => Ok(postings
                .iter()
                .find(|posting| posting.term.trim().to_lowercase() == term)
                .map(|posting| posting.ids.clone())
                .unwrap_or_default()),
        }
    }

    pub fn indexed_terms(&self) -> usize {
        match &self.directory {
            ContentArchiveDirectory::Indexed(directory) => directory.len(),
            ContentArchiveDirectory::Legacy(postings) => postings.len(),
        }
    }
}

impl MmapContentArchive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mmap = {
            // SAFETY: The returned map is read-only, owns no mutable aliases, and the
            // file handle is kept alive until map creation completes. GFM's archive
            // writers publish immutable segments via atomic rename, so readers never
            // mutate the mapped file through this API.
            unsafe { MmapOptions::new().map(&file) }.map_err(|err| GfmError::io(path, err))?
        };
        let version = mmap_content_version(&mmap, path)?;
        if version == ContentStoreVersion::Legacy {
            return Err(content_format_error(
                path,
                "legacy content archives are not mmap indexed",
            ));
        }
        verify_content_checksum_from_slice(&mmap, path, version)?;
        let directory = read_content_directory_from_slice(&mmap, path)?;
        Ok(Self {
            path: path.to_path_buf(),
            mmap,
            directory,
            version,
        })
    }

    pub fn ids_for_term(&self, term: &str) -> Result<Vec<FileId>> {
        Ok(self
            .posting_for_term(term)?
            .map(|posting| posting.ids)
            .unwrap_or_default())
    }

    pub fn posting_for_term(&self, term: &str) -> Result<Option<ContentPosting>> {
        let term = term.trim().to_lowercase();
        if term.is_empty() {
            return Ok(None);
        }
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.term.as_str().cmp(term.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok(None);
        };
        let bytes = self.posting_bytes(entry)?;
        let posting = read_content_posting(Cursor::new(bytes), &self.path, self.version)?;
        if posting.term.trim().to_lowercase() == term {
            Ok(Some(posting))
        } else {
            Err(content_format_error(
                &self.path,
                "content directory points at the wrong term",
            ))
        }
    }

    pub fn indexed_terms(&self) -> usize {
        self.directory.len()
    }

    pub fn is_checksummed(&self) -> bool {
        self.version.has_checksum()
    }

    pub fn id_block_for_term(&self, term: &str, block_index: usize) -> Result<Vec<FileId>> {
        let term = term.trim().to_lowercase();
        if term.is_empty() {
            return Ok(Vec::new());
        }
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.term.as_str().cmp(term.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok(Vec::new());
        };
        if !self.version.uses_blocked_ids() {
            return self.ids_for_term(&term);
        }
        let bytes = self.posting_bytes(entry)?;
        let mut cursor = Cursor::new(bytes);
        let decoded_term = read_content_posting_term(&mut cursor, &self.path)?;
        if decoded_term.trim().to_lowercase() != term {
            return Err(content_format_error(
                &self.path,
                "content directory points at the wrong term",
            ));
        }
        let ids_start = usize::try_from(cursor.position())
            .map_err(|_| content_format_error(&self.path, "content id offset overflow"))?;
        let id_bytes = bytes
            .get(ids_start..)
            .ok_or_else(|| content_format_error(&self.path, "content id offset out of bounds"))?;
        read_blocked_file_id_block_from_slice(id_bytes, block_index, &self.path)
    }

    pub fn mapped_len(&self) -> usize {
        self.mmap.len()
    }

    fn posting_bytes(&self, entry: &ContentDirectoryEntry) -> Result<&[u8]> {
        let start = usize::try_from(entry.offset)
            .map_err(|_| content_format_error(&self.path, "posting offset overflow"))?;
        let len = usize::try_from(entry.len)
            .map_err(|_| content_format_error(&self.path, "posting length overflow"))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| content_format_error(&self.path, "posting range overflow"))?;
        self.mmap
            .get(start..end)
            .ok_or_else(|| content_format_error(&self.path, "posting range out of bounds"))
    }
}

fn read_content_magic(mut file: impl Read, path: &Path) -> Result<Vec<u8>> {
    let mut magic = vec![0; CONTENT_MAGIC_V1.len()];
    file.read_exact(&mut magic)
        .map_err(|err| GfmError::io(path, err))?;
    if magic == CONTENT_MAGIC_V1 {
        return Ok(magic);
    }

    let mut longer = magic;
    let extra_len = CONTENT_MAGIC_V3
        .len()
        .saturating_sub(CONTENT_MAGIC_V1.len());
    if extra_len > 0 {
        let mut extra = vec![0; extra_len];
        file.read_exact(&mut extra)
            .map_err(|err| GfmError::io(path, err))?;
        longer.extend(extra);
    }
    Ok(longer)
}

fn content_version(bytes: &[u8], path: &Path) -> Result<ContentStoreVersion> {
    if bytes == CONTENT_MAGIC_V5 {
        Ok(ContentStoreVersion::IndexedChecksummed)
    } else if bytes == CONTENT_MAGIC_V4 {
        Ok(ContentStoreVersion::IndexedBlockedPositions)
    } else if bytes == CONTENT_MAGIC_V3 {
        Ok(ContentStoreVersion::IndexedPositions)
    } else if bytes == CONTENT_MAGIC_V2 {
        Ok(ContentStoreVersion::IndexedIds)
    } else if bytes == CONTENT_MAGIC_V1 {
        Ok(ContentStoreVersion::Legacy)
    } else {
        Err(GfmError::Format(format!(
            "unsupported content store header in {}",
            path.display()
        )))
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

pub fn write_content_segment(path: impl AsRef<Path>, segment: &ContentSegment) -> Result<()> {
    let path = path.as_ref();
    durable::atomic_write(path, |writer| {
        writer.write_all(CONTENT_SEGMENT_MAGIC_V2)?;
        write_file_ids(&mut *writer, &segment.tombstones)?;
        write_varint(&mut *writer, segment.postings.len() as u64)?;
        for posting in &segment.postings {
            write_content_posting(&mut *writer, posting, ContentStoreVersion::IndexedPositions)?;
        }
        Ok(())
    })
    .map(|_| ())
}

pub fn read_content_segment(path: impl AsRef<Path>) -> Result<ContentSegment> {
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    let mut magic = vec![0; CONTENT_SEGMENT_MAGIC.len()];
    file.read_exact(&mut magic)
        .map_err(|err| GfmError::io(path, err))?;
    let uses_positions = if magic == CONTENT_SEGMENT_MAGIC_V2 {
        true
    } else if magic == CONTENT_SEGMENT_MAGIC {
        false
    } else {
        return Err(GfmError::Format(format!(
            "unsupported content segment header in {}",
            path.display()
        )));
    };

    let tombstones = read_file_ids(&mut file, path)?;
    let posting_count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
    let mut postings = Vec::with_capacity(posting_count.min(1_000_000) as usize);
    for _ in 0..posting_count {
        let version = if uses_positions {
            ContentStoreVersion::IndexedPositions
        } else {
            ContentStoreVersion::IndexedIds
        };
        postings.push(read_content_posting(&mut file, path, version)?);
    }
    Ok(ContentSegment {
        tombstones,
        postings,
    })
}

pub fn compact_content_segments(
    output: impl AsRef<Path>,
    segments: &[impl AsRef<Path>],
) -> Result<Vec<ContentPosting>> {
    let mut terms: BTreeMap<String, BTreeMap<FileId, BTreeSet<u32>>> = BTreeMap::new();
    for segment_path in segments {
        let segment = read_content_segment(segment_path.as_ref())?;
        for id in segment.tombstones {
            for positions in terms.values_mut() {
                positions.remove(&id);
            }
            terms.retain(|_, positions| !positions.is_empty());
        }
        for posting in segment.postings {
            let term = posting.term.trim().to_lowercase();
            if term.is_empty() {
                continue;
            }
            let ids = terms.entry(term).or_default();
            for id in posting.ids {
                ids.entry(id).or_default();
            }
            for positions in posting.positions {
                ids.entry(positions.id)
                    .or_default()
                    .extend(positions.positions);
            }
        }
    }

    let postings: Vec<_> = terms
        .into_iter()
        .map(|(term, positions)| ContentPosting {
            term,
            ids: positions.keys().copied().collect(),
            positions: positions
                .into_iter()
                .filter(|(_, positions)| !positions.is_empty())
                .map(|(id, positions)| ContentPositions {
                    id,
                    positions: positions.into_iter().collect(),
                })
                .collect(),
        })
        .collect();
    write_content_postings(output, &postings)?;
    Ok(postings)
}

fn content_format_error(path: &Path, reason: &str) -> GfmError {
    GfmError::Format(format!(
        "invalid content store {}: {reason}",
        path.display()
    ))
}

fn verify_content_checksum_for_file(
    file: &mut File,
    path: &Path,
    version: ContentStoreVersion,
) -> Result<()> {
    if !version.has_checksum() {
        return Ok(());
    }
    let position = file
        .stream_position()
        .map_err(|err| GfmError::io(path, err))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|err| GfmError::io(path, err))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|err| GfmError::io(path, err))?;
    file.seek(SeekFrom::Start(position))
        .map_err(|err| GfmError::io(path, err))?;
    verify_content_checksum_from_slice(&bytes, path, version)
}

fn verify_content_checksum_from_slice(
    bytes: &[u8],
    path: &Path,
    version: ContentStoreVersion,
) -> Result<()> {
    if version.has_checksum()
        && !verify_checksum_footer(bytes, CONTENT_CHECKSUM_FOOTER, path, "content")?
    {
        return Err(content_format_error(
            path,
            "missing content checksum footer",
        ));
    }
    Ok(())
}

fn content_indexed_len_for_file(file: &mut File, path: &Path, len: u64) -> Result<u64> {
    let footer_len = content_checksum_footer_len() as u64;
    if len < footer_len {
        return Ok(len);
    }
    let position = file
        .stream_position()
        .map_err(|err| GfmError::io(path, err))?;
    file.seek(SeekFrom::Start(len - footer_len))
        .map_err(|err| GfmError::io(path, err))?;
    let mut footer = vec![0; footer_len as usize];
    file.read_exact(&mut footer)
        .map_err(|err| GfmError::io(path, err))?;
    file.seek(SeekFrom::Start(position))
        .map_err(|err| GfmError::io(path, err))?;
    if footer.get(4..) == Some(CONTENT_CHECKSUM_FOOTER) {
        Ok(len - footer_len)
    } else {
        Ok(len)
    }
}

fn content_indexed_len_from_slice(bytes: &[u8], path: &Path) -> Result<usize> {
    let footer_len = content_checksum_footer_len();
    if bytes.len() < footer_len {
        return Ok(bytes.len());
    }
    let footer_start = bytes.len() - footer_len;
    if bytes.get(footer_start + 4..) == Some(CONTENT_CHECKSUM_FOOTER) {
        Ok(footer_start)
    } else {
        if bytes.starts_with(CONTENT_MAGIC_V5) {
            return Err(content_format_error(
                path,
                "missing content checksum footer",
            ));
        }
        Ok(bytes.len())
    }
}

const fn content_checksum_footer_len() -> usize {
    4 + CONTENT_CHECKSUM_FOOTER.len()
}

fn write_directory_entry(
    mut writer: impl Write,
    entry: &ContentDirectoryEntry,
) -> std::io::Result<()> {
    let term = entry.term.as_bytes();
    write_varint(&mut writer, term.len() as u64)?;
    writer.write_all(term)?;
    write_varint(&mut writer, entry.offset)?;
    write_varint(&mut writer, entry.len)
}

fn read_content_directory(file: &mut File, path: &Path) -> Result<Vec<ContentDirectoryEntry>> {
    let len = file
        .metadata()
        .map_err(|err| GfmError::io(path, err))?
        .len();
    let len = content_indexed_len_for_file(file, path, len)?;
    if len < CONTENT_MAGIC_V2.len() as u64 + CONTENT_FOOTER_LEN {
        return Err(content_format_error(
            path,
            "missing content directory footer",
        ));
    }

    file.seek(SeekFrom::Start(len.saturating_sub(CONTENT_FOOTER_LEN)))
        .map_err(|err| GfmError::io(path, err))?;
    let mut offset = [0u8; 8];
    file.read_exact(&mut offset)
        .map_err(|err| GfmError::io(path, err))?;
    let directory_offset = u64::from_le_bytes(offset);
    let mut footer = vec![0; CONTENT_INDEX_FOOTER.len()];
    file.read_exact(&mut footer)
        .map_err(|err| GfmError::io(path, err))?;
    if footer != CONTENT_INDEX_FOOTER {
        return Err(content_format_error(
            path,
            "missing content directory footer",
        ));
    }
    if directory_offset >= len.saturating_sub(CONTENT_FOOTER_LEN) {
        return Err(content_format_error(
            path,
            "invalid content directory offset",
        ));
    }

    file.seek(SeekFrom::Start(directory_offset))
        .map_err(|err| GfmError::io(path, err))?;
    let count = read_varint(&mut *file).map_err(|err| GfmError::io(path, err))?;
    let mut directory = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        directory.push(read_directory_entry(&mut *file, path)?);
    }
    Ok(directory)
}

fn mmap_content_version(bytes: &[u8], path: &Path) -> Result<ContentStoreVersion> {
    if bytes.starts_with(CONTENT_MAGIC_V5) {
        Ok(ContentStoreVersion::IndexedChecksummed)
    } else if bytes.starts_with(CONTENT_MAGIC_V4) {
        Ok(ContentStoreVersion::IndexedBlockedPositions)
    } else if bytes.starts_with(CONTENT_MAGIC_V3) {
        Ok(ContentStoreVersion::IndexedPositions)
    } else if bytes.starts_with(CONTENT_MAGIC_V2) {
        Ok(ContentStoreVersion::IndexedIds)
    } else if bytes.starts_with(CONTENT_MAGIC_V1) {
        Ok(ContentStoreVersion::Legacy)
    } else {
        Err(GfmError::Format(format!(
            "unsupported content store header in {}",
            path.display()
        )))
    }
}

fn read_content_directory_from_slice(
    bytes: &[u8],
    path: &Path,
) -> Result<Vec<ContentDirectoryEntry>> {
    if bytes.len() < CONTENT_MAGIC_V2.len() + CONTENT_FOOTER_LEN as usize {
        return Err(content_format_error(
            path,
            "missing content directory footer",
        ));
    }
    let indexed_len = content_indexed_len_from_slice(bytes, path)?;
    let archive_bytes = bytes
        .get(..indexed_len)
        .ok_or_else(|| content_format_error(path, "content indexed range out of bounds"))?;
    let footer_offset = archive_bytes
        .len()
        .checked_sub(CONTENT_FOOTER_LEN as usize)
        .ok_or_else(|| content_format_error(path, "missing content directory footer"))?;
    let offset_bytes = archive_bytes
        .get(footer_offset..footer_offset + 8)
        .ok_or_else(|| content_format_error(path, "missing content directory footer"))?;
    let mut offset = [0u8; 8];
    offset.copy_from_slice(offset_bytes);
    let directory_offset = usize::try_from(u64::from_le_bytes(offset))
        .map_err(|_| content_format_error(path, "invalid content directory offset"))?;
    let footer = archive_bytes
        .get(footer_offset + 8..)
        .ok_or_else(|| content_format_error(path, "missing content directory footer"))?;
    if footer != CONTENT_INDEX_FOOTER {
        return Err(content_format_error(
            path,
            "missing content directory footer",
        ));
    }
    if directory_offset >= footer_offset {
        return Err(content_format_error(
            path,
            "invalid content directory offset",
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

fn read_directory_entry(mut reader: impl Read, path: &Path) -> Result<ContentDirectoryEntry> {
    let term_len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut term = vec![0; term_len as usize];
    reader
        .read_exact(&mut term)
        .map_err(|err| GfmError::io(path, err))?;
    let term = String::from_utf8(term).map_err(|err| {
        GfmError::Format(format!(
            "invalid UTF-8 directory term in {}: {err}",
            path.display()
        ))
    })?;
    let offset = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    Ok(ContentDirectoryEntry { term, offset, len })
}

fn write_content_posting(
    mut writer: impl Write,
    posting: &ContentPosting,
    version: ContentStoreVersion,
) -> std::io::Result<()> {
    let term = posting.term.as_bytes();
    write_varint(&mut writer, term.len() as u64)?;
    writer.write_all(term)?;
    if version.uses_blocked_ids() {
        write_blocked_file_ids(&mut writer, &posting.ids)?;
    } else {
        write_file_ids(&mut writer, &posting.ids)?;
    }
    if version.uses_positions() {
        write_content_positions(writer, &posting.positions)
    } else {
        Ok(())
    }
}

fn read_content_posting(
    mut reader: impl Read,
    path: &Path,
    version: ContentStoreVersion,
) -> Result<ContentPosting> {
    let term = read_content_posting_term(&mut reader, path)?;
    let ids = if version.uses_blocked_ids() {
        read_blocked_file_ids(&mut reader, path)?
    } else {
        read_file_ids(&mut reader, path)?
    };
    let positions = if version.uses_positions() {
        read_content_positions(reader, path)?
    } else {
        Vec::new()
    };
    Ok(ContentPosting {
        term,
        ids,
        positions,
    })
}

fn read_content_posting_term(mut reader: impl Read, path: &Path) -> Result<String> {
    let term_len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut term = vec![0; term_len as usize];
    reader
        .read_exact(&mut term)
        .map_err(|err| GfmError::io(path, err))?;
    let term = String::from_utf8(term).map_err(|err| {
        GfmError::Format(format!("invalid UTF-8 term in {}: {err}", path.display()))
    })?;
    Ok(term)
}

fn write_content_positions(
    mut writer: impl Write,
    positions: &[ContentPositions],
) -> std::io::Result<()> {
    let mut positions = positions.to_vec();
    positions.sort_by(|left, right| left.id.cmp(&right.id));
    write_varint(&mut writer, positions.len() as u64)?;
    let mut previous = FileId::new(VolumeId(0), 0);
    for entry in positions {
        write_varint(
            &mut writer,
            entry.id.volume.0.saturating_sub(previous.volume.0),
        )?;
        let node_delta = if entry.id.volume == previous.volume {
            entry.id.node.saturating_sub(previous.node)
        } else {
            entry.id.node
        };
        write_varint(&mut writer, node_delta)?;
        let mut offsets = entry.positions;
        offsets.sort_unstable();
        offsets.dedup();
        write_varint(&mut writer, offsets.len() as u64)?;
        let mut previous_position = 0u32;
        for position in offsets {
            write_varint(
                &mut writer,
                position.saturating_sub(previous_position) as u64,
            )?;
            previous_position = position;
        }
        previous = entry.id;
    }
    Ok(())
}

fn read_content_positions(mut reader: impl Read, path: &Path) -> Result<Vec<ContentPositions>> {
    let entry_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut entries = Vec::with_capacity(entry_count.min(1_000_000) as usize);
    let mut previous = FileId::new(VolumeId(0), 0);
    for _ in 0..entry_count {
        let volume_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let volume = previous
            .volume
            .0
            .checked_add(volume_delta)
            .ok_or_else(|| content_format_error(path, "position volume id overflow"))?;
        let node_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let node = if volume == previous.volume.0 {
            previous
                .node
                .checked_add(node_delta)
                .ok_or_else(|| content_format_error(path, "position file node id overflow"))?
        } else {
            node_delta
        };
        let id = FileId::new(VolumeId(volume), node);
        let position_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let mut positions = Vec::with_capacity(position_count.min(1_000_000) as usize);
        let mut previous_position = 0u32;
        for _ in 0..position_count {
            let delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
            let delta = u32::try_from(delta)
                .map_err(|_| content_format_error(path, "content position overflow"))?;
            let position = previous_position
                .checked_add(delta)
                .ok_or_else(|| content_format_error(path, "content position overflow"))?;
            positions.push(position);
            previous_position = position;
        }
        entries.push(ContentPositions { id, positions });
        previous = id;
    }
    Ok(entries)
}

fn write_file_ids(mut writer: impl Write, ids: &[FileId]) -> std::io::Result<()> {
    let mut ids = ids.to_vec();
    ids.sort();
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
            .ok_or_else(|| content_format_error(path, "volume id overflow"))?;
        let node_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let node = if volume == previous.volume.0 {
            previous
                .node
                .checked_add(node_delta)
                .ok_or_else(|| content_format_error(path, "file node id overflow"))?
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn round_trips_content_postings() {
        let path = temp_path("gfm-content-store", "idx");
        let postings = vec![
            ContentPosting {
                term: "alpha".to_string(),
                ids: vec![FileId::new(VolumeId(4), 12), FileId::new(VolumeId(4), 15)],
                positions: vec![
                    ContentPositions {
                        id: FileId::new(VolumeId(4), 12),
                        positions: vec![1, 3],
                    },
                    ContentPositions {
                        id: FileId::new(VolumeId(4), 15),
                        positions: vec![2],
                    },
                ],
            },
            ContentPosting {
                term: "beta".to_string(),
                ids: vec![FileId::new(VolumeId(5), 3)],
                positions: vec![ContentPositions {
                    id: FileId::new(VolumeId(5), 3),
                    positions: vec![0],
                }],
            },
        ];

        write_content_postings(&path, &postings).unwrap();
        let read = read_content_postings(&path).unwrap();

        assert_eq!(read, postings);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn content_archive_reads_one_term_from_directory() {
        let path = temp_path("gfm-content-archive", "gfmcontent");
        let alpha = FileId::new(VolumeId(4), 12);
        let beta = FileId::new(VolumeId(4), 15);
        write_content_postings(
            &path,
            &[
                ContentPosting {
                    term: "alpha".to_string(),
                    ids: vec![alpha],
                    positions: Vec::new(),
                },
                ContentPosting {
                    term: "beta".to_string(),
                    ids: vec![beta],
                    positions: Vec::new(),
                },
            ],
        )
        .unwrap();

        let mut archive = ContentArchive::open(&path).unwrap();

        assert_eq!(archive.indexed_terms(), 2);
        assert_eq!(archive.ids_for_term("beta").unwrap(), vec![beta]);
        assert!(archive.ids_for_term("missing").unwrap().is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_content_archive_reads_terms_without_file_seeks() {
        let path = temp_path("gfm-content-mmap-archive", "gfmcontent");
        let alpha = FileId::new(VolumeId(4), 12);
        let beta = FileId::new(VolumeId(4), 15);
        write_content_postings(
            &path,
            &[
                ContentPosting {
                    term: "alpha".to_string(),
                    ids: vec![alpha],
                    positions: vec![ContentPositions {
                        id: alpha,
                        positions: vec![1, 2],
                    }],
                },
                ContentPosting {
                    term: "beta".to_string(),
                    ids: vec![beta],
                    positions: vec![ContentPositions {
                        id: beta,
                        positions: vec![8],
                    }],
                },
            ],
        )
        .unwrap();

        let archive = MmapContentArchive::open(&path).unwrap();
        let posting = archive.posting_for_term("beta").unwrap().unwrap();

        assert_eq!(archive.indexed_terms(), 2);
        assert!(archive.mapped_len() > 0);
        assert_eq!(archive.ids_for_term("beta").unwrap(), vec![beta]);
        assert_eq!(posting.positions[0].positions, vec![8]);
        assert!(archive.ids_for_term("missing").unwrap().is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_content_archive_reads_one_compressed_id_block() {
        let path = temp_path("gfm-content-blocked-archive", "gfmcontent");
        let ids = (0..300)
            .map(|node| FileId::new(VolumeId(8), 20_000 + node))
            .collect::<Vec<_>>();
        let posting = ContentPosting {
            term: "needle".to_string(),
            ids: ids.clone(),
            positions: ids
                .iter()
                .map(|id| ContentPositions {
                    id: *id,
                    positions: vec![1, 3],
                })
                .collect(),
        };

        write_content_postings(&path, &[posting]).unwrap();
        let archive = MmapContentArchive::open(&path).unwrap();
        let block = archive.id_block_for_term("needle", 1).unwrap();
        let full = archive.posting_for_term("needle").unwrap().unwrap();

        assert_eq!(full.ids, ids);
        assert_eq!(full.positions.len(), 300);
        assert_eq!(block.len(), 128);
        assert_eq!(block[0], FileId::new(VolumeId(8), 20_128));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn checksummed_content_archive_rejects_corruption() {
        let path = temp_path("gfm-content-checksum", "gfmcontent");
        write_content_postings(
            &path,
            &[ContentPosting {
                term: "needle".to_string(),
                ids: vec![FileId::new(VolumeId(8), 20_000)],
                positions: Vec::new(),
            }],
        )
        .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let offset = bytes
            .windows(b"needle".len())
            .position(|window| window == b"needle")
            .expect("archive should contain the test term");
        bytes[offset] = b'z';
        std::fs::write(&path, bytes).unwrap();

        let read_error = read_content_postings(&path).unwrap_err().to_string();
        let mmap_error = MmapContentArchive::open(&path).unwrap_err().to_string();

        assert!(read_error.contains("checksum mismatch"), "{read_error}");
        assert!(mmap_error.contains("checksum mismatch"), "{mmap_error}");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn round_trips_content_segments() {
        let path = temp_path("gfm-content-segment", "gfmseg");
        let segment = ContentSegment {
            tombstones: vec![FileId::new(VolumeId(4), 12)],
            postings: vec![ContentPosting {
                term: "alpha".to_string(),
                ids: vec![FileId::new(VolumeId(4), 15)],
                positions: vec![ContentPositions {
                    id: FileId::new(VolumeId(4), 15),
                    positions: vec![8],
                }],
            }],
        };

        write_content_segment(&path, &segment).unwrap();
        let read = read_content_segment(&path).unwrap();

        assert_eq!(read, segment);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn compacts_content_segments_with_tombstones() {
        let first = temp_path("gfm-content-segment-first", "gfmseg");
        let second = temp_path("gfm-content-segment-second", "gfmseg");
        let output = temp_path("gfm-content-compact", "gfmcontent");
        let old = FileId::new(VolumeId(7), 10);
        let new = FileId::new(VolumeId(7), 11);

        write_content_segment(
            &first,
            &ContentSegment {
                tombstones: Vec::new(),
                postings: vec![
                    ContentPosting {
                        term: "needle".to_string(),
                        ids: vec![old],
                        positions: vec![ContentPositions {
                            id: old,
                            positions: vec![1],
                        }],
                    },
                    ContentPosting {
                        term: "stable".to_string(),
                        ids: vec![new],
                        positions: vec![ContentPositions {
                            id: new,
                            positions: vec![2],
                        }],
                    },
                ],
            },
        )
        .unwrap();
        write_content_segment(
            &second,
            &ContentSegment {
                tombstones: vec![old],
                postings: vec![ContentPosting {
                    term: "needle".to_string(),
                    ids: vec![new],
                    positions: vec![ContentPositions {
                        id: new,
                        positions: vec![3],
                    }],
                }],
            },
        )
        .unwrap();

        let compacted = compact_content_segments(&output, &[&first, &second]).unwrap();
        let reloaded = read_content_postings(&output).unwrap();

        assert_eq!(compacted, reloaded);
        assert!(reloaded
            .iter()
            .any(|posting| posting.term == "needle" && posting.ids == vec![new]));
        assert!(reloaded
            .iter()
            .any(|posting| posting.term == "stable" && posting.ids == vec![new]));
        assert!(!reloaded.iter().any(|posting| posting.ids.contains(&old)));

        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
        std::fs::remove_file(output).unwrap();
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
