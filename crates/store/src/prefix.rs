use crate::durable;
use crate::ids::{
    read_blocked_file_id_block_from_slice, read_blocked_file_ids,
    read_blocked_file_ids_for_volume_limited_from_slice_checked,
    read_blocked_file_ids_limited_from_slice_checked, write_blocked_file_ids,
};
use crate::integrity::{verify_checksum_footer, write_checksum_footer};
use gfm_types::{FileId, FileRecord, GfmError, Result, VolumeId};
use memmap2::{Mmap, MmapOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};

const PREFIX_MAGIC_V1: &[u8] = b"gfm-prefix-v1\n";
const PREFIX_INDEX_FOOTER: &[u8] = b"gfm-prefix-index-v1\n";
const PREFIX_CHECKSUM_FOOTER: &[u8] = b"gfm-prefix-checksum-v1\n";
const PREFIX_MAX_TERM_LEN: usize = 32;
const PREFIX_NORMALIZE_CHECK_STRIDE: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixPosting {
    pub prefix: String,
    pub ids: Vec<FileId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitedPrefixPosting {
    pub posting: PrefixPosting,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrefixDirectoryEntry {
    prefix: String,
    offset: u64,
    len: u64,
}

#[derive(Debug)]
pub struct MmapPrefixArchive {
    path: PathBuf,
    mmap: Mmap,
    directory: Vec<PrefixDirectoryEntry>,
}

pub fn prefix_postings_from_records(records: &[FileRecord]) -> Vec<PrefixPosting> {
    let mut postings: BTreeMap<String, BTreeSet<FileId>> = BTreeMap::new();
    for record in records {
        for token in tokenize(&normalize(&record.name)) {
            for prefix in token_prefixes(&token) {
                postings.entry(prefix).or_default().insert(record.id);
            }
        }
    }
    postings
        .into_iter()
        .map(|(prefix, ids)| PrefixPosting {
            prefix,
            ids: ids.into_iter().collect(),
        })
        .collect()
}

pub fn write_prefix_postings(path: impl AsRef<Path>, postings: &[PrefixPosting]) -> Result<()> {
    write_prefix_postings_checked(path, postings, || Ok(()))
}

pub fn write_prefix_postings_checked(
    path: impl AsRef<Path>,
    postings: &[PrefixPosting],
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    let path = path.as_ref();
    durable::atomic_write_checked(path, &mut check_control, |writer, check_control| {
        let mut bytes = Vec::new();
        {
            let mut archive = CountingWriter::new(&mut bytes);
            archive
                .write_all(PREFIX_MAGIC_V1)
                .map_err(|err| GfmError::io(path, err))?;
            write_varint(&mut archive, postings.len() as u64)
                .map_err(|err| GfmError::io(path, err))?;
            let mut postings = postings.to_vec();
            postings.sort_by(|left, right| left.prefix.cmp(&right.prefix));
            let mut directory = Vec::with_capacity(postings.len());
            for posting in &postings {
                check_control()?;
                let offset = archive.position();
                write_prefix_posting(&mut archive, posting)
                    .map_err(|err| GfmError::io(path, err))?;
                let end = archive.position();
                directory.push(PrefixDirectoryEntry {
                    prefix: posting.prefix.clone(),
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
                .write_all(PREFIX_INDEX_FOOTER)
                .map_err(|err| GfmError::io(path, err))?;
        }
        let mut footer = Vec::new();
        write_checksum_footer(&mut footer, &bytes, PREFIX_CHECKSUM_FOOTER)
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

pub fn read_prefix_postings(path: impl AsRef<Path>) -> Result<Vec<PrefixPosting>> {
    read_prefix_postings_checked(path, || Ok(()))
}

pub fn read_prefix_postings_checked(
    path: impl AsRef<Path>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<PrefixPosting>> {
    let path = path.as_ref();
    check_control()?;
    let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let mut magic = vec![0; PREFIX_MAGIC_V1.len()];
    file.read_exact(&mut magic)
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    if magic != PREFIX_MAGIC_V1 {
        return Err(prefix_format_error(path, "unsupported prefix header"));
    }
    verify_prefix_checksum_for_file_checked(&mut file, path, &mut check_control)?;
    check_control()?;
    let count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
    let mut postings = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        check_control()?;
        postings.push(read_prefix_posting(&mut file, path)?);
    }
    check_control()?;
    Ok(postings)
}

impl MmapPrefixArchive {
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
            // SAFETY: Prefix archives are immutable after atomic publication and
            // this reader only exposes bounds-checked immutable slices.
            unsafe { MmapOptions::new().map(&file) }.map_err(|err| GfmError::io(path, err))?
        };
        check_control()?;
        if mmap.get(..PREFIX_MAGIC_V1.len()) != Some(PREFIX_MAGIC_V1) {
            return Err(prefix_format_error(path, "unsupported prefix header"));
        }
        check_control()?;
        verify_prefix_checksum_from_slice(&mmap, path)?;
        check_control()?;
        let directory = read_prefix_directory_from_slice(&mmap, path)?;
        check_control()?;
        Ok(Self {
            path: path.to_path_buf(),
            mmap,
            directory,
        })
    }

    pub fn ids_for(&self, prefix: &str) -> Result<Vec<FileId>> {
        self.ids_for_checked(prefix, || Ok(()))
    }

    pub fn ids_for_checked(
        &self,
        prefix: &str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<FileId>> {
        check_control()?;
        Ok(self
            .posting_for_checked(prefix, &mut check_control)?
            .map(|posting| posting.ids)
            .unwrap_or_default())
    }

    pub fn ids_for_limit(&self, prefix: &str, limit: usize) -> Result<(Vec<FileId>, bool)> {
        self.ids_for_limit_checked(prefix, limit, || Ok(()))
    }

    pub fn ids_for_limit_checked(
        &self,
        prefix: &str,
        limit: usize,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<(Vec<FileId>, bool)> {
        check_control()?;
        let prefix = normalize_checked(prefix, &mut check_control)?;
        if prefix.is_empty() || limit == 0 {
            return Ok((Vec::new(), false));
        }
        check_control()?;
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.prefix.as_str().cmp(prefix.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok((Vec::new(), false));
        };
        check_control()?;
        let posting = self.limited_posting_for_entry_checked(entry, limit, &mut check_control)?;
        Ok((posting.posting.ids, posting.truncated))
    }

    pub fn ids_for_volume_limit(
        &self,
        prefix: &str,
        volume: VolumeId,
        limit: usize,
    ) -> Result<(Vec<FileId>, bool)> {
        self.ids_for_volume_limit_checked(prefix, volume, limit, || Ok(()))
    }

    pub fn ids_for_volume_limit_checked(
        &self,
        prefix: &str,
        volume: VolumeId,
        limit: usize,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<(Vec<FileId>, bool)> {
        check_control()?;
        let prefix = normalize_checked(prefix, &mut check_control)?;
        if prefix.is_empty() || limit == 0 {
            return Ok((Vec::new(), false));
        }
        check_control()?;
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.prefix.as_str().cmp(prefix.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok((Vec::new(), false));
        };
        check_control()?;
        let bytes = self.posting_bytes(entry)?;
        let mut cursor = Cursor::new(bytes);
        let posting_prefix = read_prefix_posting_header(&mut cursor, &self.path)?;
        if posting_prefix != entry.prefix {
            return Err(prefix_format_error(
                &self.path,
                "prefix directory points at the wrong posting",
            ));
        }
        let ids_start = usize::try_from(cursor.position())
            .map_err(|_| prefix_format_error(&self.path, "prefix id offset overflow"))?;
        let ids_bytes = bytes
            .get(ids_start..)
            .ok_or_else(|| prefix_format_error(&self.path, "prefix ids out of bounds"))?;
        check_control()?;
        read_blocked_file_ids_for_volume_limited_from_slice_checked(
            ids_bytes,
            volume,
            limit,
            &self.path,
            check_control,
        )
    }

    pub fn postings_for_sorted_prefixes_limit<I, S>(
        &self,
        prefixes: I,
        limit_per_prefix: usize,
    ) -> Result<Vec<LimitedPrefixPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.postings_for_sorted_prefixes_limit_checked(prefixes, limit_per_prefix, || Ok(()))
    }

    pub fn postings_for_sorted_prefixes_limit_checked<I, S>(
        &self,
        prefixes: I,
        limit_per_prefix: usize,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<LimitedPrefixPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        check_control()?;
        if limit_per_prefix == 0 {
            return Ok(Vec::new());
        }

        let mut postings = Vec::new();
        let mut directory_index = 0usize;
        let mut previous: Option<String> = None;

        for prefix in prefixes {
            check_control()?;
            let prefix = normalize_checked(prefix.as_ref(), &mut check_control)?;
            if prefix.is_empty() {
                continue;
            }
            if let Some(previous_prefix) = previous.as_ref() {
                if prefix < *previous_prefix {
                    return Err(prefix_format_error(
                        &self.path,
                        "batch prefix lookup terms must be sorted",
                    ));
                }
                if prefix == *previous_prefix {
                    continue;
                }
            }

            while let Some(entry) = self.directory.get(directory_index) {
                check_control()?;
                if entry.prefix.as_str() >= prefix.as_str() {
                    break;
                }
                directory_index += 1;
            }

            if let Some(entry) = self.directory.get(directory_index) {
                if entry.prefix.as_str() == prefix.as_str() {
                    check_control()?;
                    postings.push(self.limited_posting_for_entry_checked(
                        entry,
                        limit_per_prefix,
                        &mut check_control,
                    )?);
                }
            }
            previous = Some(prefix);
        }

        check_control()?;
        Ok(postings)
    }

    pub fn postings(&self) -> Result<Vec<PrefixPosting>> {
        self.directory
            .iter()
            .map(|entry| {
                let bytes = self.posting_bytes(entry)?;
                read_prefix_posting(Cursor::new(bytes), &self.path)
            })
            .collect()
    }

    pub fn postings_for<I, S>(&self, prefixes: I) -> Result<Vec<PrefixPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.postings_for_checked(prefixes, || Ok(()))
    }

    pub fn postings_for_checked<I, S>(
        &self,
        prefixes: I,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<PrefixPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();
        for prefix in prefixes {
            check_control()?;
            let prefix = normalize_checked(prefix.as_ref(), &mut check_control)?;
            if !prefix.is_empty() {
                selected.insert(prefix);
            }
        }

        selected
            .into_iter()
            .filter_map(|prefix| {
                self.posting_for_checked(&prefix, &mut check_control)
                    .transpose()
            })
            .collect()
    }

    pub fn posting_for(&self, prefix: &str) -> Result<Option<PrefixPosting>> {
        self.posting_for_checked(prefix, || Ok(()))
    }

    pub fn posting_for_checked(
        &self,
        prefix: &str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Option<PrefixPosting>> {
        check_control()?;
        let prefix = normalize_checked(prefix, &mut check_control)?;
        if prefix.is_empty() {
            return Ok(None);
        }
        check_control()?;
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.prefix.as_str().cmp(prefix.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok(None);
        };
        check_control()?;
        let bytes = self.posting_bytes(entry)?;
        let posting = read_prefix_posting(Cursor::new(bytes), &self.path)?;
        if posting.prefix == prefix {
            Ok(Some(posting))
        } else {
            Err(prefix_format_error(
                &self.path,
                "prefix directory points at the wrong posting",
            ))
        }
    }

    pub fn id_block_for(&self, prefix: &str, block_index: usize) -> Result<Vec<FileId>> {
        let prefix = normalize(prefix);
        if prefix.is_empty() {
            return Ok(Vec::new());
        }
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.prefix.as_str().cmp(prefix.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok(Vec::new());
        };
        let bytes = self.posting_bytes(entry)?;
        let mut cursor = Cursor::new(bytes);
        read_prefix_posting_header(&mut cursor, &self.path)?;
        let ids_start = usize::try_from(cursor.position())
            .map_err(|_| prefix_format_error(&self.path, "prefix id offset overflow"))?;
        let ids_bytes = bytes
            .get(ids_start..)
            .ok_or_else(|| prefix_format_error(&self.path, "prefix ids out of bounds"))?;
        read_blocked_file_id_block_from_slice(ids_bytes, block_index, &self.path)
    }

    pub fn indexed_prefixes(&self) -> usize {
        self.directory.len()
    }

    pub fn mapped_len(&self) -> usize {
        self.mmap.len()
    }

    pub fn is_checksummed(&self) -> bool {
        verify_checksum_footer(&self.mmap, PREFIX_CHECKSUM_FOOTER, &self.path, "prefix")
            .unwrap_or(false)
    }

    fn posting_bytes(&self, entry: &PrefixDirectoryEntry) -> Result<&[u8]> {
        let start = usize::try_from(entry.offset)
            .map_err(|_| prefix_format_error(&self.path, "posting offset overflow"))?;
        let len = usize::try_from(entry.len)
            .map_err(|_| prefix_format_error(&self.path, "posting length overflow"))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| prefix_format_error(&self.path, "posting range overflow"))?;
        self.mmap
            .get(start..end)
            .ok_or_else(|| prefix_format_error(&self.path, "posting range out of bounds"))
    }

    fn limited_posting_for_entry_checked(
        &self,
        entry: &PrefixDirectoryEntry,
        limit: usize,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<LimitedPrefixPosting> {
        check_control()?;
        let bytes = self.posting_bytes(entry)?;
        let mut cursor = Cursor::new(bytes);
        let posting_prefix = read_prefix_posting_header(&mut cursor, &self.path)?;
        if posting_prefix != entry.prefix {
            return Err(prefix_format_error(
                &self.path,
                "prefix directory points at the wrong posting",
            ));
        }
        let ids_start = usize::try_from(cursor.position())
            .map_err(|_| prefix_format_error(&self.path, "prefix id offset overflow"))?;
        let ids_bytes = bytes
            .get(ids_start..)
            .ok_or_else(|| prefix_format_error(&self.path, "prefix ids out of bounds"))?;
        check_control()?;
        let mut ids = read_blocked_file_ids_limited_from_slice_checked(
            ids_bytes,
            limit.saturating_add(1),
            &self.path,
            &mut check_control,
        )?;
        let truncated = ids.len() > limit;
        ids.truncate(limit);
        Ok(LimitedPrefixPosting {
            posting: PrefixPosting {
                prefix: posting_prefix,
                ids,
            },
            truncated,
        })
    }
}

fn write_prefix_posting(mut writer: impl Write, posting: &PrefixPosting) -> std::io::Result<()> {
    let prefix = normalize(&posting.prefix);
    write_varint(&mut writer, prefix.len() as u64)?;
    writer.write_all(prefix.as_bytes())?;
    write_blocked_file_ids(&mut writer, &posting.ids)
}

fn read_prefix_posting(mut reader: impl Read, path: &Path) -> Result<PrefixPosting> {
    let prefix = read_prefix_posting_header(&mut reader, path)?;
    let ids = read_blocked_file_ids(reader, path)?;
    Ok(PrefixPosting { prefix, ids })
}

fn read_prefix_posting_header(mut reader: impl Read, path: &Path) -> Result<String> {
    let prefix_len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let prefix_len = usize::try_from(prefix_len)
        .map_err(|_| prefix_format_error(path, "prefix length overflow"))?;
    let mut prefix = vec![0; prefix_len];
    reader
        .read_exact(&mut prefix)
        .map_err(|err| GfmError::io(path, err))?;
    String::from_utf8(prefix).map_err(|err| {
        prefix_format_error(path, &format!("invalid UTF-8 prefix in archive: {err}"))
    })
}

fn write_directory_entry(
    mut writer: impl Write,
    entry: &PrefixDirectoryEntry,
) -> std::io::Result<()> {
    write_varint(&mut writer, entry.prefix.len() as u64)?;
    writer.write_all(entry.prefix.as_bytes())?;
    writer.write_all(&entry.offset.to_le_bytes())?;
    writer.write_all(&entry.len.to_le_bytes())
}

fn read_prefix_directory_from_slice(
    bytes: &[u8],
    path: &Path,
) -> Result<Vec<PrefixDirectoryEntry>> {
    let indexed_len = prefix_indexed_len_from_slice(bytes, path)?;
    let footer_start = indexed_len
        .checked_sub(PREFIX_INDEX_FOOTER.len())
        .and_then(|value| value.checked_sub(8))
        .ok_or_else(|| prefix_format_error(path, "missing prefix directory footer"))?;
    let mut directory_offset = [0u8; 8];
    directory_offset.copy_from_slice(
        bytes
            .get(footer_start..footer_start + 8)
            .ok_or_else(|| prefix_format_error(path, "missing prefix directory footer"))?,
    );
    let directory_offset = usize::try_from(u64::from_le_bytes(directory_offset))
        .map_err(|_| prefix_format_error(path, "invalid prefix directory offset"))?;
    if directory_offset >= footer_start {
        return Err(prefix_format_error(path, "invalid prefix directory offset"));
    }
    let mut reader = Cursor::new(
        bytes
            .get(directory_offset..footer_start)
            .ok_or_else(|| prefix_format_error(path, "prefix directory out of bounds"))?,
    );
    let count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut directory = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        let prefix_len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let prefix_len = usize::try_from(prefix_len)
            .map_err(|_| prefix_format_error(path, "prefix directory term length overflow"))?;
        let mut prefix = vec![0; prefix_len];
        reader
            .read_exact(&mut prefix)
            .map_err(|err| GfmError::io(path, err))?;
        let prefix = String::from_utf8(prefix).map_err(|err| {
            prefix_format_error(path, &format!("invalid UTF-8 prefix in directory: {err}"))
        })?;
        let mut offset = [0u8; 8];
        reader
            .read_exact(&mut offset)
            .map_err(|err| GfmError::io(path, err))?;
        let mut len = [0u8; 8];
        reader
            .read_exact(&mut len)
            .map_err(|err| GfmError::io(path, err))?;
        directory.push(PrefixDirectoryEntry {
            prefix,
            offset: u64::from_le_bytes(offset),
            len: u64::from_le_bytes(len),
        });
    }
    directory.sort_by(|left, right| left.prefix.cmp(&right.prefix));
    Ok(directory)
}

fn verify_prefix_checksum_for_file_checked(
    file: &mut File,
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    const CHUNK_BYTES: usize = 256 * 1024;

    check_control()?;
    let data_start = PREFIX_MAGIC_V1.len() as u64;
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
    verify_prefix_checksum_from_slice(&full, path)?;
    file.seek(std::io::SeekFrom::Start(data_start))
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    Ok(())
}

fn verify_prefix_checksum_from_slice(bytes: &[u8], path: &Path) -> Result<()> {
    if !verify_checksum_footer(bytes, PREFIX_CHECKSUM_FOOTER, path, "prefix")? {
        return Err(prefix_format_error(path, "missing prefix checksum footer"));
    }
    Ok(())
}

fn prefix_indexed_len_from_slice(bytes: &[u8], path: &Path) -> Result<usize> {
    let footer_len = 4usize
        .checked_add(PREFIX_CHECKSUM_FOOTER.len())
        .ok_or_else(|| prefix_format_error(path, "prefix checksum footer length overflow"))?;
    if bytes.len() < footer_len {
        return Err(prefix_format_error(path, "missing prefix checksum footer"));
    }
    let indexed_len = bytes.len() - footer_len;
    if bytes.get(indexed_len + 4..) != Some(PREFIX_CHECKSUM_FOOTER) {
        return Err(prefix_format_error(path, "missing prefix checksum footer"));
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
        if index.is_multiple_of(PREFIX_NORMALIZE_CHECK_STRIDE) {
            check_control()?;
        }
        normalized.push(ch.to_ascii_lowercase());
    }
    check_control()?;
    Ok(normalized)
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn token_prefixes(term: &str) -> impl Iterator<Item = String> + '_ {
    term.char_indices()
        .map(|(index, _)| index)
        .skip(1)
        .chain(std::iter::once(term.len()))
        .take(PREFIX_MAX_TERM_LEN)
        .map(|end| term[..end].to_string())
}

fn prefix_format_error(path: &Path, reason: &str) -> GfmError {
    GfmError::Format(format!("invalid prefix store {}: {reason}", path.display()))
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
    fn mmap_prefix_archive_reads_one_compressed_id_block() {
        let path = temp_path("gfm-prefix-blocked", "gfmprefix");
        let posting = PrefixPosting {
            prefix: "pro".to_string(),
            ids: (0..300)
                .map(|node| FileId::new(VolumeId(5), 10_000 + node))
                .collect(),
        };

        write_prefix_postings(&path, std::slice::from_ref(&posting)).unwrap();
        let archive = MmapPrefixArchive::open(&path).unwrap();
        let ids = archive.ids_for("pro").unwrap();
        let block = archive.id_block_for("pro", 1).unwrap();
        let (limited, truncated) = archive.ids_for_limit("pro", 129).unwrap();
        let (all_limited, all_truncated) = archive.ids_for_limit("pro", 400).unwrap();

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
    fn full_prefix_reader_round_trips_after_checksum_validation() {
        let path = temp_path("gfm-prefix-full-read", "gfmprefix");
        let postings = vec![
            PrefixPosting {
                prefix: "pro".to_string(),
                ids: vec![FileId::new(VolumeId(1), 2)],
            },
            PrefixPosting {
                prefix: "proj".to_string(),
                ids: vec![FileId::new(VolumeId(1), 4), FileId::new(VolumeId(2), 1)],
            },
        ];

        write_prefix_postings(&path, &postings).unwrap();
        let read = read_prefix_postings(&path).unwrap();

        assert_eq!(read, postings);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn checked_prefix_reader_honors_pre_cancelled_control_before_file_open() {
        let path = temp_path("gfm-prefix-read-cancel", "gfmprefix");
        let result = read_prefix_postings_checked(&path, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!path.exists());
    }

    #[test]
    fn mmap_prefix_archive_limits_ids_by_volume_without_global_hydration() {
        let path = temp_path("gfm-prefix-volume-limit", "gfmprefix");
        let posting = PrefixPosting {
            prefix: "pro".to_string(),
            ids: (0..192)
                .map(|node| FileId::new(VolumeId(1), 1_000 + node))
                .chain((0..192).map(|node| FileId::new(VolumeId(2), 2_000 + node)))
                .chain((0..32).map(|node| FileId::new(VolumeId(3), 3_000 + node)))
                .collect(),
        };

        write_prefix_postings(&path, std::slice::from_ref(&posting)).unwrap();
        let archive = MmapPrefixArchive::open(&path).unwrap();
        let (volume_two, truncated) = archive
            .ids_for_volume_limit("pro", VolumeId(2), 130)
            .unwrap();
        let (missing, missing_truncated) = archive
            .ids_for_volume_limit("pro", VolumeId(9), 130)
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
    fn mmap_prefix_archive_reads_limited_ids_for_one_volume() {
        let path = temp_path("gfm-prefix-volume-blocked", "gfmprefix");
        let posting = PrefixPosting {
            prefix: "pro".to_string(),
            ids: (0..140)
                .map(|node| FileId::new(VolumeId(1), 1_000 + node))
                .chain((0..140).map(|node| FileId::new(VolumeId(2), 2_000 + node)))
                .chain((0..5).map(|node| FileId::new(VolumeId(3), 3_000 + node)))
                .collect(),
        };

        write_prefix_postings(&path, std::slice::from_ref(&posting)).unwrap();
        let archive = MmapPrefixArchive::open(&path).unwrap();
        let (volume_two, volume_two_truncated) = archive
            .ids_for_volume_limit("PRO", VolumeId(2), 129)
            .unwrap();
        let (volume_three, volume_three_truncated) = archive
            .ids_for_volume_limit("pro", VolumeId(3), 129)
            .unwrap();
        let (missing, missing_truncated) = archive
            .ids_for_volume_limit("pro", VolumeId(9), 129)
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
    fn mmap_prefix_archive_reads_all_postings_for_startup_import() {
        let path = temp_path("gfm-prefix-postings", "gfmprefix");
        let postings = vec![
            PrefixPosting {
                prefix: "pro".to_string(),
                ids: vec![FileId::new(VolumeId(1), 1), FileId::new(VolumeId(2), 2)],
            },
            PrefixPosting {
                prefix: "proj".to_string(),
                ids: vec![FileId::new(VolumeId(1), 1)],
            },
        ];

        write_prefix_postings(&path, &postings).unwrap();
        let archive = MmapPrefixArchive::open(&path).unwrap();

        assert_eq!(archive.postings().unwrap(), postings);
        assert_eq!(
            archive.postings_for(["proj", "missing", "proj"]).unwrap(),
            vec![postings[1].clone()]
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_prefix_archive_reads_bounded_sorted_postings_in_one_pass() {
        let path = temp_path("gfm-prefix-batch-postings", "gfmprefix");
        let postings = vec![
            PrefixPosting {
                prefix: "alpha".to_string(),
                ids: (0..4)
                    .map(|node| FileId::new(VolumeId(1), 100 + node))
                    .collect(),
            },
            PrefixPosting {
                prefix: "project".to_string(),
                ids: (0..6)
                    .map(|node| FileId::new(VolumeId(1), 200 + node))
                    .collect(),
            },
        ];

        write_prefix_postings(&path, &postings).unwrap();
        let archive = MmapPrefixArchive::open(&path).unwrap();
        let batch = archive
            .postings_for_sorted_prefixes_limit(["alpha", "alpha", "missing", "project"], 3)
            .unwrap();

        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].posting.prefix, "alpha");
        assert_eq!(batch[0].posting.ids, postings[0].ids[..3]);
        assert!(batch[0].truncated);
        assert_eq!(batch[1].posting.prefix, "project");
        assert_eq!(batch[1].posting.ids, postings[1].ids[..3]);
        assert!(batch[1].truncated);
        assert!(archive
            .postings_for_sorted_prefixes_limit(["project", "alpha"], 3)
            .is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_prefix_archive_checked_lookup_honors_pre_cancelled_control() {
        let path = temp_path("gfm-prefix-checked-lookup-cancel", "gfmprefix");
        write_prefix_postings(
            &path,
            &[PrefixPosting {
                prefix: "project".to_string(),
                ids: vec![FileId::new(VolumeId(1), 42)],
            }],
        )
        .unwrap();
        let archive = MmapPrefixArchive::open(&path).unwrap();

        assert!(matches!(
            archive.ids_for_checked("project", || Err(GfmError::Cancelled)),
            Err(GfmError::Cancelled)
        ));
        assert!(matches!(
            archive.postings_for_sorted_prefixes_limit_checked(["project"], 8, || {
                Err(GfmError::Cancelled)
            }),
            Err(GfmError::Cancelled)
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_prefix_archive_checked_lookup_can_cancel_during_normalization() {
        let path = temp_path("gfm-prefix-checked-normalize-cancel", "gfmprefix");
        write_prefix_postings(
            &path,
            &[PrefixPosting {
                prefix: "project".to_string(),
                ids: vec![FileId::new(VolumeId(1), 42)],
            }],
        )
        .unwrap();
        let archive = MmapPrefixArchive::open(&path).unwrap();
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
    fn prefix_postings_from_records_include_name_prefixes() {
        let first = record(1, "/tmp/project-plan.md", "project-plan.md");
        let second = record(2, "/tmp/profile.txt", "profile.txt");
        let postings = prefix_postings_from_records(&[first, second]);

        let pro = postings
            .iter()
            .find(|posting| posting.prefix == "pro")
            .unwrap();
        let proj = postings
            .iter()
            .find(|posting| posting.prefix == "proj")
            .unwrap();

        assert_eq!(pro.ids.len(), 2);
        assert_eq!(proj.ids.len(), 1);
    }

    #[test]
    fn mmap_prefix_archive_checked_open_honors_pre_cancelled_control_before_file_open() {
        let path = temp_path("gfm-prefix-open-cancel", "gfmprefix");

        let result = MmapPrefixArchive::open_checked(&path, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!path.exists());
    }

    #[test]
    fn checksummed_prefix_archive_rejects_corruption() {
        let path = temp_path("gfm-prefix-checksum", "gfmprefix");
        write_prefix_postings(
            &path,
            &[PrefixPosting {
                prefix: "pro".to_string(),
                ids: vec![FileId::new(VolumeId(1), 2)],
            }],
        )
        .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let index = PREFIX_MAGIC_V1.len() + 2;
        bytes[index] ^= 0x10;
        std::fs::write(&path, bytes).unwrap();

        let read_error = read_prefix_postings(&path).unwrap_err().to_string();
        let mmap_error = MmapPrefixArchive::open(&path).unwrap_err().to_string();

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
