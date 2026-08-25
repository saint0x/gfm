use crate::durable;
use crate::ids::{
    read_blocked_file_id_block_from_slice, read_blocked_file_ids,
    read_blocked_file_ids_limited_from_slice, write_blocked_file_ids,
};
use crate::integrity::{verify_checksum_footer, write_checksum_footer};
use gfm_types::{FileId, FileRecord, GfmError, Result};
use memmap2::{Mmap, MmapOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};

const SUBSTRING_MAGIC_V1: &[u8] = b"gfm-substring-v1\n";
const SUBSTRING_INDEX_FOOTER: &[u8] = b"gfm-substring-index-v1\n";
const SUBSTRING_CHECKSUM_FOOTER: &[u8] = b"gfm-substring-checksum-v1\n";
const SUBSTRING_GRAM_CHARS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubstringPosting {
    pub gram: String,
    pub ids: Vec<FileId>,
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
    let path = path.as_ref();
    durable::atomic_write(path, |writer| {
        let mut bytes = Vec::new();
        {
            let mut archive = CountingWriter::new(&mut bytes);
            archive.write_all(SUBSTRING_MAGIC_V1)?;
            write_varint(&mut archive, postings.len() as u64)?;
            let mut postings = postings.to_vec();
            postings.sort_by(|left, right| left.gram.cmp(&right.gram));
            let mut directory = Vec::with_capacity(postings.len());
            for posting in &postings {
                let offset = archive.position();
                write_substring_posting(&mut archive, posting)?;
                let end = archive.position();
                directory.push(SubstringDirectoryEntry {
                    gram: posting.gram.clone(),
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
            archive.write_all(SUBSTRING_INDEX_FOOTER)?;
        }
        let mut footer = Vec::new();
        write_checksum_footer(&mut footer, &bytes, SUBSTRING_CHECKSUM_FOOTER)?;
        bytes.extend(footer);
        writer.write_all(&bytes)?;
        Ok(())
    })
    .map(|_| ())
}

pub fn read_substring_postings(path: impl AsRef<Path>) -> Result<Vec<SubstringPosting>> {
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    let mut magic = vec![0; SUBSTRING_MAGIC_V1.len()];
    file.read_exact(&mut magic)
        .map_err(|err| GfmError::io(path, err))?;
    if magic != SUBSTRING_MAGIC_V1 {
        return Err(substring_format_error(path, "unsupported substring header"));
    }
    verify_substring_checksum_for_file(&mut file, path)?;
    let count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
    let mut postings = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        postings.push(read_substring_posting(&mut file, path)?);
    }
    Ok(postings)
}

impl MmapSubstringArchive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mmap = {
            // SAFETY: Substring archives are immutable after atomic publication and
            // this reader only exposes bounds-checked immutable slices.
            unsafe { MmapOptions::new().map(&file) }.map_err(|err| GfmError::io(path, err))?
        };
        if mmap.get(..SUBSTRING_MAGIC_V1.len()) != Some(SUBSTRING_MAGIC_V1) {
            return Err(substring_format_error(path, "unsupported substring header"));
        }
        verify_substring_checksum_from_slice(&mmap, path)?;
        let directory = read_substring_directory_from_slice(&mmap, path)?;
        Ok(Self {
            path: path.to_path_buf(),
            mmap,
            directory,
        })
    }

    pub fn ids_for(&self, gram: &str) -> Result<Vec<FileId>> {
        Ok(self
            .posting_for(gram)?
            .map(|posting| posting.ids)
            .unwrap_or_default())
    }

    pub fn ids_for_limit(&self, gram: &str, limit: usize) -> Result<(Vec<FileId>, bool)> {
        let gram = normalize(gram);
        if !is_substring_gram(&gram) || limit == 0 {
            return Ok((Vec::new(), false));
        }
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.gram.as_str().cmp(gram.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok((Vec::new(), false));
        };
        let bytes = self.posting_bytes(entry)?;
        let mut cursor = Cursor::new(bytes);
        let posting_gram = read_substring_posting_header(&mut cursor, &self.path)?;
        if posting_gram != gram {
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
        Ok((ids, truncated))
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
        let mut selected = BTreeSet::new();
        for gram in grams {
            let gram = normalize(gram.as_ref());
            if is_substring_gram(&gram) {
                selected.insert(gram);
            }
        }

        selected
            .into_iter()
            .filter_map(|gram| self.posting_for(&gram).transpose())
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
        let mut selected = BTreeSet::new();
        for gram in grams {
            let gram = normalize(gram.as_ref());
            if is_substring_gram(&gram) {
                selected.insert(gram);
            }
        }

        let mut postings = Vec::new();
        for gram in selected {
            let (ids, _) = self.ids_for_limit(&gram, limit_per_gram)?;
            if !ids.is_empty() {
                postings.push(SubstringPosting { gram, ids });
            }
        }
        Ok(postings)
    }

    pub fn posting_for(&self, gram: &str) -> Result<Option<SubstringPosting>> {
        let gram = normalize(gram);
        if !is_substring_gram(&gram) {
            return Ok(None);
        }
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.gram.as_str().cmp(gram.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok(None);
        };
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

fn verify_substring_checksum_for_file(file: &mut File, path: &Path) -> Result<()> {
    let mut full = Vec::new();
    file.rewind().map_err(|err| GfmError::io(path, err))?;
    file.read_to_end(&mut full)
        .map_err(|err| GfmError::io(path, err))?;
    verify_substring_checksum_from_slice(&full, path)
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
