use crate::durable;
use crate::integrity::{verify_checksum_footer, write_checksum_footer};
use codec::{
    read_blocked_id_block, read_content_posting,
    read_content_posting_for_volume_limited_from_slice, read_content_posting_limited_from_slice,
    read_content_posting_term, write_content_posting, write_file_ids, write_varint,
};
pub(crate) use codec::{read_file_ids, read_varint};
use gfm_types::{ContentPosting, ContentSegment, FileId, GfmError, Result, VolumeId};
use memmap2::{Mmap, MmapOptions};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

mod codec;

const CONTENT_MAGIC_V1: &[u8] = b"gfm-content-v1\n";
const CONTENT_MAGIC_V2: &[u8] = b"gfm-content-v2\n";
const CONTENT_MAGIC_V3: &[u8] = b"gfm-content-v3\n";
const CONTENT_MAGIC_V4: &[u8] = b"gfm-content-v4\n";
const CONTENT_MAGIC_V5: &[u8] = b"gfm-content-v5\n";
pub(crate) const CONTENT_SEGMENT_MAGIC: &[u8] = b"gfm-content-segment-v1\n";
pub(crate) const CONTENT_SEGMENT_MAGIC_V2: &[u8] = b"gfm-content-segment-v2\n";
const CONTENT_INDEX_FOOTER: &[u8] = b"gfm-content-index-v1\n";
const CONTENT_CHECKSUM_FOOTER: &[u8] = b"gfm-content-checksum-v1\n";
const CONTENT_FOOTER_LEN: u64 = 8 + CONTENT_INDEX_FOOTER.len() as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContentStoreVersion {
    Legacy,
    IndexedIds,
    IndexedPositions,
    IndexedBlockedPositions,
    IndexedChecksummed,
}

impl ContentStoreVersion {
    pub(super) const fn uses_positions(self) -> bool {
        matches!(
            self,
            Self::IndexedPositions | Self::IndexedBlockedPositions | Self::IndexedChecksummed
        )
    }

    pub(super) const fn uses_blocked_ids(self) -> bool {
        matches!(
            self,
            Self::IndexedBlockedPositions | Self::IndexedChecksummed
        )
    }

    pub(super) const fn has_checksum(self) -> bool {
        matches!(self, Self::IndexedChecksummed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitedContentPosting {
    pub posting: ContentPosting,
    pub truncated: bool,
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
    read_content_postings_checked(path, || Ok(()))
}

pub fn read_content_postings_checked(
    path: impl AsRef<Path>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ContentPosting>> {
    let path = path.as_ref();
    check_control()?;
    let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let magic = read_content_magic(&mut file, path)?;
    check_control()?;
    let version = content_version(&magic, path)?;
    if version == ContentStoreVersion::Legacy && magic != CONTENT_MAGIC_V1 {
        return Err(GfmError::Format(format!(
            "unsupported content store header in {}",
            path.display()
        )));
    }
    verify_content_checksum_for_file_checked(&mut file, path, version, &mut check_control)?;
    check_control()?;

    let count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
    let mut postings = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        check_control()?;
        postings.push(read_content_posting(&mut file, path, version)?);
    }
    check_control()?;
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
        Self::open_checked(path, || Ok(()))
    }

    pub fn open_checked(
        path: impl AsRef<Path>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let path = path.as_ref();
        check_control()?;
        let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        check_control()?;
        let magic = read_content_magic(&mut file, path)?;
        check_control()?;
        let version = content_version(&magic, path)?;
        check_control()?;
        if matches!(
            version,
            ContentStoreVersion::IndexedIds
                | ContentStoreVersion::IndexedPositions
                | ContentStoreVersion::IndexedBlockedPositions
                | ContentStoreVersion::IndexedChecksummed
        ) {
            verify_content_checksum_for_file_checked(&mut file, path, version, &mut check_control)?;
            check_control()?;
            let directory = read_content_directory_checked(&mut file, path, &mut check_control)?;
            check_control()?;
            Ok(Self {
                path: path.to_path_buf(),
                file,
                directory: ContentArchiveDirectory::Indexed(directory),
                version,
            })
        } else if version == ContentStoreVersion::Legacy {
            file.seek(SeekFrom::Start(CONTENT_MAGIC_V1.len() as u64))
                .map_err(|err| GfmError::io(path, err))?;
            check_control()?;
            let count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
            let mut postings = Vec::with_capacity(count.min(1_000_000) as usize);
            for _ in 0..count {
                check_control()?;
                postings.push(read_content_posting(&mut file, path, version)?);
            }
            check_control()?;
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
            // SAFETY: The returned map is read-only, owns no mutable aliases, and the
            // file handle is kept alive until map creation completes. GFM's archive
            // writers publish immutable segments via atomic rename, so readers never
            // mutate the mapped file through this API.
            unsafe { MmapOptions::new().map(&file) }.map_err(|err| GfmError::io(path, err))?
        };
        check_control()?;
        let version = mmap_content_version(&mmap, path)?;
        check_control()?;
        if version == ContentStoreVersion::Legacy {
            return Err(content_format_error(
                path,
                "legacy content archives are not mmap indexed",
            ));
        }
        check_control()?;
        verify_content_checksum_from_slice(&mmap, path, version)?;
        check_control()?;
        let directory = read_content_directory_from_slice(&mmap, path)?;
        check_control()?;
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

    pub fn posting_for_term_limit(
        &self,
        term: &str,
        limit: usize,
    ) -> Result<(Option<ContentPosting>, bool)> {
        let term = term.trim().to_lowercase();
        if term.is_empty() {
            return Ok((None, false));
        }
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.term.as_str().cmp(term.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok((None, false));
        };
        let bytes = self.posting_bytes(entry)?;
        let (posting, truncated) =
            read_content_posting_limited_from_slice(bytes, &self.path, self.version, limit)?;
        if posting.term.trim().to_lowercase() == term {
            Ok((Some(posting), truncated))
        } else {
            Err(content_format_error(
                &self.path,
                "content directory points at the wrong term",
            ))
        }
    }

    pub fn postings_for_terms<I, S>(&self, terms: I) -> Result<Vec<ContentPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();
        for term in terms {
            let term = term.as_ref().trim().to_lowercase();
            if !term.is_empty() {
                selected.insert(term);
            }
        }

        selected
            .into_iter()
            .filter_map(|term| self.posting_for_term(&term).transpose())
            .collect()
    }

    pub fn postings_for_terms_limit<I, S>(
        &self,
        terms: I,
        limit_per_term: usize,
    ) -> Result<Vec<ContentPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();
        for term in terms {
            let term = term.as_ref().trim().to_lowercase();
            if !term.is_empty() {
                selected.insert(term);
            }
        }

        self.postings_for_sorted_terms_limit(selected, limit_per_term)
            .map(|postings| {
                postings
                    .into_iter()
                    .map(|posting| posting.posting)
                    .collect()
            })
    }

    pub fn postings_for_sorted_terms_limit<I, S>(
        &self,
        terms: I,
        limit_per_term: usize,
    ) -> Result<Vec<LimitedContentPosting>>
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
            let term = term.as_ref().trim().to_lowercase();
            if term.is_empty() {
                continue;
            }
            if let Some(previous_term) = previous.as_ref() {
                if term < *previous_term {
                    return Err(content_format_error(
                        &self.path,
                        "batch content lookup terms must be sorted",
                    ));
                }
                if term == *previous_term {
                    continue;
                }
            }

            while let Some(entry) = self.directory.get(directory_index) {
                if entry.term.as_str() >= term.as_str() {
                    break;
                }
                directory_index += 1;
            }

            if let Some(entry) = self.directory.get(directory_index) {
                if entry.term.as_str() == term.as_str() {
                    let bytes = self.posting_bytes(entry)?;
                    let (posting, truncated) = read_content_posting_limited_from_slice(
                        bytes,
                        &self.path,
                        self.version,
                        limit_per_term,
                    )?;
                    if posting.term.trim().to_lowercase() == term {
                        postings.push(LimitedContentPosting { posting, truncated });
                    } else {
                        return Err(content_format_error(
                            &self.path,
                            "content directory points at the wrong term",
                        ));
                    }
                }
            }
            previous = Some(term);
        }

        Ok(postings)
    }

    pub fn postings_for_sorted_terms_volume_limit<I, S>(
        &self,
        terms: I,
        volume: VolumeId,
        limit_per_term: usize,
    ) -> Result<Vec<LimitedContentPosting>>
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
            let term = term.as_ref().trim().to_lowercase();
            if term.is_empty() {
                continue;
            }
            if let Some(previous_term) = previous.as_ref() {
                if term < *previous_term {
                    return Err(content_format_error(
                        &self.path,
                        "batch content volume lookup terms must be sorted",
                    ));
                }
                if term == *previous_term {
                    continue;
                }
            }

            while let Some(entry) = self.directory.get(directory_index) {
                if entry.term.as_str() >= term.as_str() {
                    break;
                }
                directory_index += 1;
            }

            if let Some(entry) = self.directory.get(directory_index) {
                if entry.term.as_str() == term.as_str() {
                    let bytes = self.posting_bytes(entry)?;
                    let (posting, truncated) = read_content_posting_for_volume_limited_from_slice(
                        bytes,
                        &self.path,
                        self.version,
                        volume,
                        limit_per_term,
                    )?;
                    if posting.term.trim().to_lowercase() == term {
                        if !posting.ids.is_empty() || !posting.positions.is_empty() {
                            postings.push(LimitedContentPosting { posting, truncated });
                        }
                    } else {
                        return Err(content_format_error(
                            &self.path,
                            "content directory points at the wrong term",
                        ));
                    }
                }
            }
            previous = Some(term);
        }

        Ok(postings)
    }

    pub fn postings(&self) -> Result<Vec<ContentPosting>> {
        self.directory
            .iter()
            .map(|entry| {
                let bytes = self.posting_bytes(entry)?;
                read_content_posting(Cursor::new(bytes), &self.path, self.version)
            })
            .collect()
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
        read_blocked_id_block(id_bytes, block_index, &self.path)
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
    read_content_segment_checked(path, || Ok(()))
}

pub fn read_content_segment_checked(
    path: impl AsRef<Path>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ContentSegment> {
    let path = path.as_ref();
    check_control()?;
    let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let mut magic = vec![0; CONTENT_SEGMENT_MAGIC.len()];
    file.read_exact(&mut magic)
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
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
    check_control()?;
    let posting_count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
    let mut postings = Vec::with_capacity(posting_count.min(1_000_000) as usize);
    for _ in 0..posting_count {
        check_control()?;
        let version = if uses_positions {
            ContentStoreVersion::IndexedPositions
        } else {
            ContentStoreVersion::IndexedIds
        };
        postings.push(read_content_posting(&mut file, path, version)?);
    }
    check_control()?;
    Ok(ContentSegment {
        tombstones,
        postings,
    })
}

pub(crate) fn content_format_error(path: &Path, reason: &str) -> GfmError {
    GfmError::Format(format!(
        "invalid content store {}: {reason}",
        path.display()
    ))
}

fn verify_content_checksum_for_file_checked(
    file: &mut File,
    path: &Path,
    version: ContentStoreVersion,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    if !version.has_checksum() {
        return Ok(());
    }
    const CHUNK_BYTES: usize = 256 * 1024;

    check_control()?;
    let position = file
        .stream_position()
        .map_err(|err| GfmError::io(path, err))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let mut bytes = Vec::new();
    let mut buffer = [0; CHUNK_BYTES];
    loop {
        check_control()?;
        let len = file
            .read(&mut buffer)
            .map_err(|err| GfmError::io(path, err))?;
        if len == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..len]);
    }
    check_control()?;
    file.seek(SeekFrom::Start(position))
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
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

fn content_indexed_len_for_file_checked(
    file: &mut File,
    path: &Path,
    len: u64,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<u64> {
    let footer_len = content_checksum_footer_len() as u64;
    if len < footer_len {
        return Ok(len);
    }
    check_control()?;
    let position = file
        .stream_position()
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    file.seek(SeekFrom::Start(len - footer_len))
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let mut footer = vec![0; footer_len as usize];
    file.read_exact(&mut footer)
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    file.seek(SeekFrom::Start(position))
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
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

fn read_content_directory_checked(
    file: &mut File,
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ContentDirectoryEntry>> {
    check_control()?;
    let len = file
        .metadata()
        .map_err(|err| GfmError::io(path, err))?
        .len();
    check_control()?;
    let len = content_indexed_len_for_file_checked(file, path, len, &mut check_control)?;
    if len < CONTENT_MAGIC_V2.len() as u64 + CONTENT_FOOTER_LEN {
        return Err(content_format_error(
            path,
            "missing content directory footer",
        ));
    }

    check_control()?;
    file.seek(SeekFrom::Start(len.saturating_sub(CONTENT_FOOTER_LEN)))
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let mut offset = [0u8; 8];
    file.read_exact(&mut offset)
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let directory_offset = u64::from_le_bytes(offset);
    let mut footer = vec![0; CONTENT_INDEX_FOOTER.len()];
    file.read_exact(&mut footer)
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
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
    check_control()?;
    let count = read_varint(&mut *file).map_err(|err| GfmError::io(path, err))?;
    let mut directory = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        check_control()?;
        directory.push(read_directory_entry(&mut *file, path)?);
    }
    check_control()?;
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

#[cfg(test)]
mod tests;
