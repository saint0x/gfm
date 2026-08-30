use crate::durable;
use crate::ids::{
    read_blocked_file_id_block_from_slice, read_blocked_file_ids,
    read_blocked_file_ids_for_volume_limited_from_slice, read_blocked_file_ids_limited_from_slice,
    write_blocked_file_ids,
};
use crate::integrity::{verify_checksum_footer, write_checksum_footer};
use gfm_types::{FileId, FileRecord, GfmError, Result, VolumeId};
use memmap2::{Mmap, MmapOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};

const SUBSTRING_MAGIC_V1: &[u8] = b"gfm-substring-v1\n";
const SUBSTRING_INDEX_FOOTER: &[u8] = b"gfm-substring-index-v1\n";
const SUBSTRING_CHECKSUM_FOOTER: &[u8] = b"gfm-substring-checksum-v1\n";
const SUBSTRING_GRAM_CHARS: usize = 3;
const SUBSTRING_NORMALIZE_CHECK_STRIDE: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubstringPosting {
    pub gram: String,
    pub ids: Vec<FileId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitedSubstringPosting {
    pub posting: SubstringPosting,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SubstringDirectoryEntry {
    gram: String,
    offset: u64,
    len: u64,
}

#[derive(Debug)]
pub struct MmapSubstringArchive {
    path: PathBuf,
    mmap: Mmap,
    directory: Vec<SubstringDirectoryEntry>,
}

pub fn substring_postings_from_records(records: &[FileRecord]) -> Vec<SubstringPosting> {
    let mut postings: BTreeMap<String, BTreeSet<FileId>> = BTreeMap::new();
    for record in records {
        for gram in substring_grams(&normalize(&record.name)) {
            postings.entry(gram).or_default().insert(record.id);
        }
    }
    postings
        .into_iter()
        .map(|(gram, ids)| SubstringPosting {
            gram,
            ids: ids.into_iter().collect(),
        })
        .collect()
}

pub fn write_substring_postings(
    path: impl AsRef<Path>,
    postings: &[SubstringPosting],
) -> Result<()> {
    write_substring_postings_checked(path, postings, || Ok(()))
}

pub fn write_substring_postings_checked(
    path: impl AsRef<Path>,
    postings: &[SubstringPosting],
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    let path = path.as_ref();
    durable::atomic_write_checked(path, &mut check_control, |writer, check_control| {
        let mut bytes = Vec::new();
        {
            let mut archive = CountingWriter::new(&mut bytes);
            archive
                .write_all(SUBSTRING_MAGIC_V1)
                .map_err(|err| GfmError::io(path, err))?;
            write_varint(&mut archive, postings.len() as u64)
                .map_err(|err| GfmError::io(path, err))?;
            let mut postings = postings.to_vec();
            postings.sort_by(|left, right| left.gram.cmp(&right.gram));
            let mut directory = Vec::with_capacity(postings.len());
            for posting in &postings {
                check_control()?;
                let offset = archive.position();
                write_substring_posting(&mut archive, posting)
                    .map_err(|err| GfmError::io(path, err))?;
                let end = archive.position();
                directory.push(SubstringDirectoryEntry {
                    gram: posting.gram.clone(),
                    offset,
                    len: end.saturating_sub(offset),
                });
            }
            let directory_offset = archive.position();
            write_varint(&mut archive, directory.len() as u64)
                .map_err(|err| GfmError::io(path, err))?;
            for entry in &directory {
                check_control()?;
                write_directory_entry(&mut archive, entry)
                    .map_err(|err| GfmError::io(path, err))?;
            }
            check_control()?;
            archive
                .write_all(&directory_offset.to_le_bytes())
                .map_err(|err| GfmError::io(path, err))?;
            archive
                .write_all(SUBSTRING_INDEX_FOOTER)
                .map_err(|err| GfmError::io(path, err))?;
        }
        let mut footer = Vec::new();
        write_checksum_footer(&mut footer, &bytes, SUBSTRING_CHECKSUM_FOOTER)
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

pub fn read_substring_postings(path: impl AsRef<Path>) -> Result<Vec<SubstringPosting>> {
    read_substring_postings_checked(path, || Ok(()))
}

pub fn read_substring_postings_checked(
    path: impl AsRef<Path>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<SubstringPosting>> {
    let path = path.as_ref();
    check_control()?;
    let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let mut magic = vec![0; SUBSTRING_MAGIC_V1.len()];
    file.read_exact(&mut magic)
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    if magic != SUBSTRING_MAGIC_V1 {
        return Err(substring_format_error(path, "unsupported substring header"));
    }
    verify_substring_checksum_for_file_checked(&mut file, path, &mut check_control)?;
    check_control()?;
    let count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
    let mut postings = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        check_control()?;
        postings.push(read_substring_posting(&mut file, path)?);
    }
    check_control()?;
    Ok(postings)
}

impl MmapSubstringArchive {
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
            // SAFETY: Substring archives are immutable after atomic publication and
            // this reader only exposes bounds-checked immutable slices.
            unsafe { MmapOptions::new().map(&file) }.map_err(|err| GfmError::io(path, err))?
        };
        check_control()?;
        if mmap.get(..SUBSTRING_MAGIC_V1.len()) != Some(SUBSTRING_MAGIC_V1) {
            return Err(substring_format_error(path, "unsupported substring header"));
        }
        check_control()?;
        verify_substring_checksum_from_slice(&mmap, path)?;
        check_control()?;
        let directory = read_substring_directory_from_slice(&mmap, path)?;
        check_control()?;
        Ok(Self {
            path: path.to_path_buf(),
            mmap,
            directory,
        })
    }

    pub fn ids_for(&self, gram: &str) -> Result<Vec<FileId>> {
        self.ids_for_checked(gram, || Ok(()))
    }

    pub fn ids_for_checked(
        &self,
        gram: &str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<FileId>> {
        check_control()?;
        Ok(self
            .posting_for_checked(gram, &mut check_control)?
            .map(|posting| posting.ids)
            .unwrap_or_default())
    }

    pub fn ids_for_limit(&self, gram: &str, limit: usize) -> Result<(Vec<FileId>, bool)> {
        self.ids_for_limit_checked(gram, limit, || Ok(()))
    }

    pub fn ids_for_limit_checked(
        &self,
        gram: &str,
        limit: usize,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<(Vec<FileId>, bool)> {
        check_control()?;
        let gram = normalize_checked(gram, &mut check_control)?;
        if !is_substring_gram(&gram) || limit == 0 {
            return Ok((Vec::new(), false));
        }
        check_control()?;
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.gram.as_str().cmp(gram.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok((Vec::new(), false));
        };
        check_control()?;
        let posting = self.limited_posting_for_entry(entry, limit)?;
        Ok((posting.posting.ids, posting.truncated))
    }

    pub fn ids_for_volume_limit(
        &self,
        gram: &str,
        volume: VolumeId,
        limit: usize,
    ) -> Result<(Vec<FileId>, bool)> {
        self.ids_for_volume_limit_checked(gram, volume, limit, || Ok(()))
    }

    pub fn ids_for_volume_limit_checked(
        &self,
        gram: &str,
        volume: VolumeId,
        limit: usize,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<(Vec<FileId>, bool)> {
        check_control()?;
        let gram = normalize_checked(gram, &mut check_control)?;
        if !is_substring_gram(&gram) || limit == 0 {
            return Ok((Vec::new(), false));
        }
        check_control()?;
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.gram.as_str().cmp(gram.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok((Vec::new(), false));
        };
        check_control()?;
        let bytes = self.posting_bytes(entry)?;
        let mut cursor = Cursor::new(bytes);
        let posting_gram = read_substring_posting_header(&mut cursor, &self.path)?;
        if posting_gram != entry.gram {
            return Err(substring_format_error(
                &self.path,
                "substring directory points at the wrong posting",
            ));
        }
        let ids_start = usize::try_from(cursor.position())
            .map_err(|_| substring_format_error(&self.path, "substring id offset overflow"))?;
        let ids_bytes = bytes
            .get(ids_start..)
            .ok_or_else(|| substring_format_error(&self.path, "substring ids out of bounds"))?;
        check_control()?;
        read_blocked_file_ids_for_volume_limited_from_slice(ids_bytes, volume, limit, &self.path)
    }

    pub fn postings_for_sorted_grams_limit<I, S>(
        &self,
        grams: I,
        limit_per_gram: usize,
    ) -> Result<Vec<LimitedSubstringPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.postings_for_sorted_grams_limit_checked(grams, limit_per_gram, || Ok(()))
    }

    pub fn postings_for_sorted_grams_limit_checked<I, S>(
        &self,
        grams: I,
        limit_per_gram: usize,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<LimitedSubstringPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        check_control()?;
        if limit_per_gram == 0 {
            return Ok(Vec::new());
        }

        let mut postings = Vec::new();
        let mut directory_index = 0usize;
        let mut previous: Option<String> = None;

        for gram in grams {
            check_control()?;
            let gram = normalize_checked(gram.as_ref(), &mut check_control)?;
            if !is_substring_gram(&gram) {
                continue;
            }
            if let Some(previous_gram) = previous.as_ref() {
                if gram < *previous_gram {
                    return Err(substring_format_error(
                        &self.path,
                        "batch substring lookup grams must be sorted",
                    ));
                }
                if gram == *previous_gram {
                    continue;
                }
            }

            while let Some(entry) = self.directory.get(directory_index) {
                check_control()?;
                if entry.gram.as_str() >= gram.as_str() {
                    break;
                }
                directory_index += 1;
            }

            if let Some(entry) = self.directory.get(directory_index) {
                if entry.gram.as_str() == gram.as_str() {
                    check_control()?;
                    postings.push(self.limited_posting_for_entry(entry, limit_per_gram)?);
                }
            }
            previous = Some(gram);
        }

        check_control()?;
        Ok(postings)
    }

    pub fn postings(&self) -> Result<Vec<SubstringPosting>> {
        self.directory
            .iter()
            .map(|entry| {
                let bytes = self.posting_bytes(entry)?;
                read_substring_posting(Cursor::new(bytes), &self.path)
            })
            .collect()
    }

    pub fn postings_for<I, S>(&self, grams: I) -> Result<Vec<SubstringPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.postings_for_checked(grams, || Ok(()))
    }

    pub fn postings_for_checked<I, S>(
        &self,
        grams: I,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<SubstringPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();
        for gram in grams {
            check_control()?;
            let gram = normalize_checked(gram.as_ref(), &mut check_control)?;
            if is_substring_gram(&gram) {
                selected.insert(gram);
            }
        }

        selected
            .into_iter()
            .filter_map(|gram| {
                self.posting_for_checked(&gram, &mut check_control)
                    .transpose()
            })
            .collect()
    }

    pub fn postings_for_limit<I, S>(
        &self,
        grams: I,
        limit_per_gram: usize,
    ) -> Result<Vec<SubstringPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.postings_for_limit_checked(grams, limit_per_gram, || Ok(()))
    }

    pub fn postings_for_limit_checked<I, S>(
        &self,
        grams: I,
        limit_per_gram: usize,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<SubstringPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();
        for gram in grams {
            check_control()?;
            let gram = normalize_checked(gram.as_ref(), &mut check_control)?;
            if is_substring_gram(&gram) {
                selected.insert(gram);
            }
        }

        self.postings_for_sorted_grams_limit_checked(selected, limit_per_gram, check_control)
            .map(|postings| {
                postings
                    .into_iter()
                    .map(|posting| posting.posting)
                    .collect()
            })
    }

    pub fn posting_for(&self, gram: &str) -> Result<Option<SubstringPosting>> {
        self.posting_for_checked(gram, || Ok(()))
    }

    pub fn posting_for_checked(
        &self,
        gram: &str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Option<SubstringPosting>> {
        check_control()?;
        let gram = normalize_checked(gram, &mut check_control)?;
        if !is_substring_gram(&gram) {
            return Ok(None);
        }
        check_control()?;
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.gram.as_str().cmp(gram.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok(None);
        };
        check_control()?;
        let bytes = self.posting_bytes(entry)?;
        let posting = read_substring_posting(Cursor::new(bytes), &self.path)?;
        if posting.gram == gram {
            Ok(Some(posting))
        } else {
            Err(substring_format_error(
                &self.path,
                "substring directory points at the wrong posting",
            ))
        }
    }

    pub fn id_block_for(&self, gram: &str, block_index: usize) -> Result<Vec<FileId>> {
        let gram = normalize(gram);
        if !is_substring_gram(&gram) {
            return Ok(Vec::new());
        }
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.gram.as_str().cmp(gram.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok(Vec::new());
        };
        let bytes = self.posting_bytes(entry)?;
        let mut cursor = Cursor::new(bytes);
        read_substring_posting_header(&mut cursor, &self.path)?;
        let ids_start = usize::try_from(cursor.position())
            .map_err(|_| substring_format_error(&self.path, "substring id offset overflow"))?;
        let ids_bytes = bytes
            .get(ids_start..)
            .ok_or_else(|| substring_format_error(&self.path, "substring ids out of bounds"))?;
        read_blocked_file_id_block_from_slice(ids_bytes, block_index, &self.path)
    }

    pub fn indexed_grams(&self) -> usize {
        self.directory.len()
    }

    pub fn mapped_len(&self) -> usize {
        self.mmap.len()
    }

    pub fn is_checksummed(&self) -> bool {
        verify_checksum_footer(
            &self.mmap,
            SUBSTRING_CHECKSUM_FOOTER,
            &self.path,
            "substring",
        )
        .unwrap_or(false)
    }

    fn posting_bytes(&self, entry: &SubstringDirectoryEntry) -> Result<&[u8]> {
        let start = usize::try_from(entry.offset)
            .map_err(|_| substring_format_error(&self.path, "posting offset overflow"))?;
        let len = usize::try_from(entry.len)
            .map_err(|_| substring_format_error(&self.path, "posting length overflow"))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| substring_format_error(&self.path, "posting range overflow"))?;
        self.mmap
            .get(start..end)
            .ok_or_else(|| substring_format_error(&self.path, "posting range out of bounds"))
    }

    fn limited_posting_for_entry(
        &self,
        entry: &SubstringDirectoryEntry,
        limit: usize,
    ) -> Result<LimitedSubstringPosting> {
        let bytes = self.posting_bytes(entry)?;
        let mut cursor = Cursor::new(bytes);
        let posting_gram = read_substring_posting_header(&mut cursor, &self.path)?;
        if posting_gram != entry.gram {
            return Err(substring_format_error(
                &self.path,
                "substring directory points at the wrong posting",
            ));
        }
        let ids_start = usize::try_from(cursor.position())
            .map_err(|_| substring_format_error(&self.path, "substring id offset overflow"))?;
        let ids_bytes = bytes
            .get(ids_start..)
            .ok_or_else(|| substring_format_error(&self.path, "substring ids out of bounds"))?;
        let mut ids = read_blocked_file_ids_limited_from_slice(
            ids_bytes,
            limit.saturating_add(1),
            &self.path,
        )?;
        let truncated = ids.len() > limit;
        ids.truncate(limit);
        Ok(LimitedSubstringPosting {
            posting: SubstringPosting {
                gram: posting_gram,
                ids,
            },
            truncated,
        })
    }
}

fn write_substring_posting(
    mut writer: impl Write,
    posting: &SubstringPosting,
) -> std::io::Result<()> {
    let gram = normalize(&posting.gram);
    write_varint(&mut writer, gram.len() as u64)?;
    writer.write_all(gram.as_bytes())?;
    write_blocked_file_ids(&mut writer, &posting.ids)
}

fn read_substring_posting(mut reader: impl Read, path: &Path) -> Result<SubstringPosting> {
    let gram = read_substring_posting_header(&mut reader, path)?;
    let ids = read_blocked_file_ids(reader, path)?;
    Ok(SubstringPosting { gram, ids })
}

fn read_substring_posting_header(mut reader: impl Read, path: &Path) -> Result<String> {
    let gram_len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let gram_len = usize::try_from(gram_len)
        .map_err(|_| substring_format_error(path, "substring length overflow"))?;
    let mut gram = vec![0; gram_len];
    reader
        .read_exact(&mut gram)
        .map_err(|err| GfmError::io(path, err))?;
    String::from_utf8(gram).map_err(|err| {
        substring_format_error(
            path,
            &format!("invalid UTF-8 substring gram in archive: {err}"),
        )
    })
}

fn write_directory_entry(
    mut writer: impl Write,
    entry: &SubstringDirectoryEntry,
) -> std::io::Result<()> {
    write_varint(&mut writer, entry.gram.len() as u64)?;
    writer.write_all(entry.gram.as_bytes())?;
    writer.write_all(&entry.offset.to_le_bytes())?;
    writer.write_all(&entry.len.to_le_bytes())
}

fn read_substring_directory_from_slice(
    bytes: &[u8],
    path: &Path,
) -> Result<Vec<SubstringDirectoryEntry>> {
    let indexed_len = substring_indexed_len_from_slice(bytes, path)?;
    let footer_start = indexed_len
        .checked_sub(SUBSTRING_INDEX_FOOTER.len())
        .and_then(|value| value.checked_sub(8))
        .ok_or_else(|| substring_format_error(path, "missing substring directory footer"))?;
    let mut directory_offset = [0u8; 8];
    directory_offset.copy_from_slice(
        bytes
            .get(footer_start..footer_start + 8)
            .ok_or_else(|| substring_format_error(path, "missing substring directory footer"))?,
    );
    let directory_offset = usize::try_from(u64::from_le_bytes(directory_offset))
        .map_err(|_| substring_format_error(path, "invalid substring directory offset"))?;
    if directory_offset >= footer_start {
        return Err(substring_format_error(
            path,
            "invalid substring directory offset",
        ));
    }
    let mut reader = Cursor::new(
        bytes
            .get(directory_offset..footer_start)
            .ok_or_else(|| substring_format_error(path, "substring directory out of bounds"))?,
    );
    let count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut directory = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        let gram_len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let gram_len = usize::try_from(gram_len).map_err(|_| {
            substring_format_error(path, "substring directory gram length overflow")
        })?;
        let mut gram = vec![0; gram_len];
        reader
            .read_exact(&mut gram)
            .map_err(|err| GfmError::io(path, err))?;
        let gram = String::from_utf8(gram).map_err(|err| {
            substring_format_error(path, &format!("invalid UTF-8 gram in directory: {err}"))
        })?;
        let mut offset = [0u8; 8];
        reader
            .read_exact(&mut offset)
            .map_err(|err| GfmError::io(path, err))?;
        let mut len = [0u8; 8];
        reader
            .read_exact(&mut len)
            .map_err(|err| GfmError::io(path, err))?;
        directory.push(SubstringDirectoryEntry {
            gram,
            offset: u64::from_le_bytes(offset),
            len: u64::from_le_bytes(len),
        });
    }
    directory.sort_by(|left, right| left.gram.cmp(&right.gram));
    Ok(directory)
}

fn verify_substring_checksum_for_file_checked(
    file: &mut File,
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    const CHUNK_BYTES: usize = 256 * 1024;

    check_control()?;
    let data_start = SUBSTRING_MAGIC_V1.len() as u64;
    let mut full = Vec::new();
    file.rewind().map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let mut buffer = [0; CHUNK_BYTES];
    loop {
        check_control()?;
        let len = file
            .read(&mut buffer)
            .map_err(|err| GfmError::io(path, err))?;
        if len == 0 {
            break;
        }
        full.extend_from_slice(&buffer[..len]);
    }
    check_control()?;
    verify_substring_checksum_from_slice(&full, path)?;
    file.seek(std::io::SeekFrom::Start(data_start))
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    Ok(())
}

fn verify_substring_checksum_from_slice(bytes: &[u8], path: &Path) -> Result<()> {
    if !verify_checksum_footer(bytes, SUBSTRING_CHECKSUM_FOOTER, path, "substring")? {
        return Err(substring_format_error(
            path,
            "missing substring checksum footer",
        ));
    }
    Ok(())
}

fn substring_indexed_len_from_slice(bytes: &[u8], path: &Path) -> Result<usize> {
    let footer_len = 4usize
        .checked_add(SUBSTRING_CHECKSUM_FOOTER.len())
        .ok_or_else(|| substring_format_error(path, "substring checksum footer length overflow"))?;
    if bytes.len() < footer_len {
        return Err(substring_format_error(
            path,
            "missing substring checksum footer",
        ));
    }
    let indexed_len = bytes.len() - footer_len;
    if bytes.get(indexed_len + 4..) != Some(SUBSTRING_CHECKSUM_FOOTER) {
        return Err(substring_format_error(
            path,
            "missing substring checksum footer",
        ));
    }
    Ok(indexed_len)
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn normalize_checked(value: &str, mut check_control: impl FnMut() -> Result<()>) -> Result<String> {
    check_control()?;
    let mut normalized = String::new();
    for (index, ch) in value.trim().chars().enumerate() {
        if index.is_multiple_of(SUBSTRING_NORMALIZE_CHECK_STRIDE) {
            check_control()?;
        }
        normalized.push(ch.to_ascii_lowercase());
    }
    check_control()?;
    Ok(normalized)
}

fn substring_grams(value: &str) -> Vec<String> {
    let mut starts = value
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    starts.push(value.len());
    if starts.len() <= SUBSTRING_GRAM_CHARS {
        return Vec::new();
    }
    let mut grams = starts
        .windows(SUBSTRING_GRAM_CHARS + 1)
        .map(|window| value[window[0]..window[SUBSTRING_GRAM_CHARS]].to_string())
        .collect::<Vec<_>>();
    grams.sort();
    grams.dedup();
    grams
}

fn is_substring_gram(value: &str) -> bool {
    value.chars().count() == SUBSTRING_GRAM_CHARS
}

fn substring_format_error(path: &Path, reason: &str) -> GfmError {
    GfmError::Format(format!(
        "invalid substring store {}: {reason}",
        path.display()
    ))
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

struct CountingWriter<W> {
    inner: W,
    position: u64,
}

impl<W> CountingWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner, position: 0 }
    }

    const fn position(&self) -> u64 {
        self.position
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.position += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::{FileKind, VolumeId};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn mmap_substring_archive_reads_one_compressed_id_block() {
        let path = temp_path("gfm-substring-blocked", "gfmsubstr");
        let posting = SubstringPosting {
            gram: "por".to_string(),
            ids: (0..300)
                .map(|node| FileId::new(VolumeId(5), 10_000 + node))
                .collect(),
        };

        write_substring_postings(&path, std::slice::from_ref(&posting)).unwrap();
        let archive = MmapSubstringArchive::open(&path).unwrap();
        let ids = archive.ids_for("POR").unwrap();
        let block = archive.id_block_for("por", 1).unwrap();
        let (limited, truncated) = archive.ids_for_limit("POR", 129).unwrap();
        let (all_limited, all_truncated) = archive.ids_for_limit("por", 400).unwrap();

        assert_eq!(ids, posting.ids);
        assert_eq!(block.len(), 128);
        assert_eq!(block[0], FileId::new(VolumeId(5), 10_128));
        assert!(truncated);
        assert_eq!(limited.len(), 129);
        assert_eq!(limited[0], FileId::new(VolumeId(5), 10_000));
        assert_eq!(limited[128], FileId::new(VolumeId(5), 10_128));
        assert!(!all_truncated);
        assert_eq!(all_limited, posting.ids);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn full_substring_reader_round_trips_after_checksum_validation() {
        let path = temp_path("gfm-substring-full-read", "gfmsubstr");
        let postings = vec![
            SubstringPosting {
                gram: "pro".to_string(),
                ids: vec![FileId::new(VolumeId(1), 2)],
            },
            SubstringPosting {
                gram: "roj".to_string(),
                ids: vec![FileId::new(VolumeId(1), 4), FileId::new(VolumeId(2), 1)],
            },
        ];

        write_substring_postings(&path, &postings).unwrap();
        let read = read_substring_postings(&path).unwrap();

        assert_eq!(read, postings);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn checked_substring_reader_honors_pre_cancelled_control_before_file_open() {
        let path = temp_path("gfm-substring-read-cancel", "gfmsubstr");
        let result = read_substring_postings_checked(&path, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!path.exists());
    }

    #[test]
    fn mmap_substring_archive_limits_ids_by_volume_without_global_hydration() {
        let path = temp_path("gfm-substring-volume-limit", "gfmsubstr");
        let posting = SubstringPosting {
            gram: "por".to_string(),
            ids: (0..192)
                .map(|node| FileId::new(VolumeId(1), 1_000 + node))
                .chain((0..192).map(|node| FileId::new(VolumeId(2), 2_000 + node)))
                .chain((0..32).map(|node| FileId::new(VolumeId(3), 3_000 + node)))
                .collect(),
        };

        write_substring_postings(&path, std::slice::from_ref(&posting)).unwrap();
        let archive = MmapSubstringArchive::open(&path).unwrap();
        let (volume_two, truncated) = archive
            .ids_for_volume_limit("POR", VolumeId(2), 130)
            .unwrap();
        let (missing, missing_truncated) = archive
            .ids_for_volume_limit("por", VolumeId(9), 130)
            .unwrap();

        assert!(truncated);
        assert_eq!(volume_two.len(), 130);
        assert_eq!(volume_two[0], FileId::new(VolumeId(2), 2_000));
        assert_eq!(volume_two[129], FileId::new(VolumeId(2), 2_129));
        assert!(missing.is_empty());
        assert!(!missing_truncated);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_substring_archive_reads_limited_ids_for_one_volume() {
        let path = temp_path("gfm-substring-volume-blocked", "gfmsubstr");
        let posting = SubstringPosting {
            gram: "por".to_string(),
            ids: (0..140)
                .map(|node| FileId::new(VolumeId(1), 1_000 + node))
                .chain((0..140).map(|node| FileId::new(VolumeId(2), 2_000 + node)))
                .chain((0..5).map(|node| FileId::new(VolumeId(3), 3_000 + node)))
                .collect(),
        };

        write_substring_postings(&path, std::slice::from_ref(&posting)).unwrap();
        let archive = MmapSubstringArchive::open(&path).unwrap();
        let (volume_two, volume_two_truncated) = archive
            .ids_for_volume_limit("POR", VolumeId(2), 129)
            .unwrap();
        let (volume_three, volume_three_truncated) = archive
            .ids_for_volume_limit("por", VolumeId(3), 129)
            .unwrap();
        let (missing, missing_truncated) = archive
            .ids_for_volume_limit("por", VolumeId(9), 129)
            .unwrap();

        assert_eq!(volume_two.len(), 129);
        assert_eq!(volume_two[0], FileId::new(VolumeId(2), 2_000));
        assert_eq!(volume_two[128], FileId::new(VolumeId(2), 2_128));
        assert!(volume_two_truncated);
        assert_eq!(
            volume_three,
            (0..5)
                .map(|node| FileId::new(VolumeId(3), 3_000 + node))
                .collect::<Vec<_>>()
        );
        assert!(!volume_three_truncated);
        assert!(missing.is_empty());
        assert!(!missing_truncated);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_substring_archive_reads_selected_postings_for_startup_import() {
        let path = temp_path("gfm-substring-postings", "gfmsubstr");
        let postings = vec![
            SubstringPosting {
                gram: "por".to_string(),
                ids: vec![FileId::new(VolumeId(1), 1), FileId::new(VolumeId(2), 2)],
            },
            SubstringPosting {
                gram: "rep".to_string(),
                ids: vec![FileId::new(VolumeId(1), 1)],
            },
        ];

        write_substring_postings(&path, &postings).unwrap();
        let archive = MmapSubstringArchive::open(&path).unwrap();

        assert_eq!(archive.postings().unwrap(), postings);
        assert_eq!(
            archive.postings_for(["rep", "missing", "REP"]).unwrap(),
            vec![postings[1].clone()]
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_substring_archive_reads_bounded_selected_postings_for_query_import() {
        let path = temp_path("gfm-substring-bounded-postings", "gfmsubstr");
        let postings = vec![
            SubstringPosting {
                gram: "por".to_string(),
                ids: (0..8)
                    .map(|node| FileId::new(VolumeId(1), node + 1))
                    .collect(),
            },
            SubstringPosting {
                gram: "rep".to_string(),
                ids: vec![FileId::new(VolumeId(2), 1), FileId::new(VolumeId(2), 2)],
            },
        ];

        write_substring_postings(&path, &postings).unwrap();
        let archive = MmapSubstringArchive::open(&path).unwrap();
        let limited = archive
            .postings_for_limit(["REP", "missing", "por", "por"], 3)
            .unwrap();

        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].gram, "por");
        assert_eq!(limited[0].ids.len(), 3);
        assert_eq!(limited[0].ids[0], FileId::new(VolumeId(1), 1));
        assert_eq!(limited[1], postings[1]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_substring_archive_reads_bounded_sorted_postings_in_one_pass() {
        let path = temp_path("gfm-substring-batch-postings", "gfmsubstr");
        let postings = vec![
            SubstringPosting {
                gram: "alp".to_string(),
                ids: (0..4)
                    .map(|node| FileId::new(VolumeId(1), 100 + node))
                    .collect(),
            },
            SubstringPosting {
                gram: "pro".to_string(),
                ids: (0..6)
                    .map(|node| FileId::new(VolumeId(1), 200 + node))
                    .collect(),
            },
        ];

        write_substring_postings(&path, &postings).unwrap();
        let archive = MmapSubstringArchive::open(&path).unwrap();
        let batch = archive
            .postings_for_sorted_grams_limit(["alp", "alp", "bad", "missing", "pro"], 3)
            .unwrap();

        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].posting.gram, "alp");
        assert_eq!(batch[0].posting.ids, postings[0].ids[..3]);
        assert!(batch[0].truncated);
        assert_eq!(batch[1].posting.gram, "pro");
        assert_eq!(batch[1].posting.ids, postings[1].ids[..3]);
        assert!(batch[1].truncated);
        assert!(archive
            .postings_for_sorted_grams_limit(["pro", "alp"], 3)
            .is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_substring_archive_checked_lookup_honors_pre_cancelled_control() {
        let path = temp_path("gfm-substring-checked-lookup-cancel", "gfmsubstr");
        write_substring_postings(
            &path,
            &[SubstringPosting {
                gram: "pro".to_string(),
                ids: vec![FileId::new(VolumeId(1), 42)],
            }],
        )
        .unwrap();
        let archive = MmapSubstringArchive::open(&path).unwrap();

        assert!(matches!(
            archive.ids_for_checked("pro", || Err(GfmError::Cancelled)),
            Err(GfmError::Cancelled)
        ));
        assert!(matches!(
            archive.postings_for_limit_checked(["pro"], 8, || Err(GfmError::Cancelled)),
            Err(GfmError::Cancelled)
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_substring_archive_checked_lookup_can_cancel_during_normalization() {
        let path = temp_path("gfm-substring-checked-normalize-cancel", "gfmsubstr");
        write_substring_postings(
            &path,
            &[SubstringPosting {
                gram: "pro".to_string(),
                ids: vec![FileId::new(VolumeId(1), 42)],
            }],
        )
        .unwrap();
        let archive = MmapSubstringArchive::open(&path).unwrap();
        let mut checks = 0usize;

        let result = archive.ids_for_limit_checked(&"P".repeat(1_024), 8, || {
            checks += 1;
            if checks >= 3 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert_eq!(checks, 3);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn substring_postings_from_records_include_name_trigrams() {
        let postings = substring_postings_from_records(&[
            record(1, "/tmp/report.pdf", "report.pdf"),
            record(2, "/tmp/notes.txt", "notes.txt"),
        ]);

        let por = postings
            .iter()
            .find(|posting| posting.gram == "por")
            .unwrap();
        let ote = postings
            .iter()
            .find(|posting| posting.gram == "ote")
            .unwrap();

        assert_eq!(por.ids, vec![FileId::new(VolumeId(1), 1)]);
        assert_eq!(ote.ids, vec![FileId::new(VolumeId(1), 2)]);
    }

    #[test]
    fn mmap_substring_archive_checked_open_honors_pre_cancelled_control_before_file_open() {
        let path = temp_path("gfm-substring-open-cancel", "gfmsubstr");

        let result = MmapSubstringArchive::open_checked(&path, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!path.exists());
    }

    #[test]
    fn checksummed_substring_archive_rejects_corruption() {
        let path = temp_path("gfm-substring-checksum", "gfmsubstr");
        write_substring_postings(
            &path,
            &[SubstringPosting {
                gram: "por".to_string(),
                ids: vec![FileId::new(VolumeId(1), 2)],
            }],
        )
        .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let index = SUBSTRING_MAGIC_V1.len() + 2;
        bytes[index] ^= 0x10;
        std::fs::write(&path, bytes).unwrap();

        let read_error = read_substring_postings(&path).unwrap_err().to_string();
        let mmap_error = MmapSubstringArchive::open(&path).unwrap_err().to_string();

        assert!(read_error.contains("checksum mismatch"));
        assert!(mmap_error.contains("checksum mismatch"));
        std::fs::remove_file(path).unwrap();
    }

    fn record(node: u64, path: &str, name: &str) -> FileRecord {
        FileRecord {
            id: FileId::new(VolumeId(1), node),
            parent: None,
            path: PathBuf::from(path),
            name: name.to_string(),
            kind: FileKind::File,
            len: 0,
            created: Some(UNIX_EPOCH),
            modified: Some(SystemTime::now()),
            changed: Some(SystemTime::now()),
            mode: 0o644,
            owner: 501,
            group: 20,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
            xattrs_digest: 0,
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
