use crate::durable;
use crate::integrity::{verify_checksum_footer, write_checksum_footer};
use gfm_types::{FileRecord, GfmError, Result};
use memmap2::{Mmap, MmapOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};

const FUZZY_MAGIC_V1: &[u8] = b"gfm-fuzzy-v1\n";
const FUZZY_INDEX_FOOTER: &[u8] = b"gfm-fuzzy-index-v1\n";
const FUZZY_CHECKSUM_FOOTER: &[u8] = b"gfm-fuzzy-checksum-v1\n";
const FUZZY_MIN_TERM_LEN: usize = 2;
const FUZZY_MAX_TERM_LEN: usize = 32;
const FUZZY_MAX_DELETIONS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyPosting {
    pub key: String,
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitedFuzzyPosting {
    pub posting: FuzzyPosting,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FuzzyDirectoryEntry {
    key: String,
    offset: u64,
    len: u64,
}

#[derive(Debug)]
pub struct MmapFuzzyArchive {
    path: PathBuf,
    mmap: Mmap,
    directory: Vec<FuzzyDirectoryEntry>,
}

pub fn fuzzy_postings_from_records(records: &[FileRecord]) -> Vec<FuzzyPosting> {
    let mut terms = BTreeSet::new();
    for record in records {
        for token in tokenize(&normalize(&record.name)) {
            if is_fuzzy_term(&token) {
                terms.insert(token);
            }
        }
    }
    let mut postings: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for token in terms {
        for key in deletion_keys(&token, FUZZY_MAX_DELETIONS) {
            postings.entry(key).or_default().insert(token.clone());
        }
    }
    postings
        .into_iter()
        .map(|(key, terms)| FuzzyPosting {
            key,
            terms: terms.into_iter().collect(),
        })
        .collect()
}

pub fn write_fuzzy_postings(path: impl AsRef<Path>, postings: &[FuzzyPosting]) -> Result<()> {
    let path = path.as_ref();
    durable::atomic_write(path, |writer| {
        let mut bytes = Vec::new();
        {
            let mut archive = CountingWriter::new(&mut bytes);
            archive.write_all(FUZZY_MAGIC_V1)?;
            write_varint(&mut archive, postings.len() as u64)?;
            let mut postings = postings.to_vec();
            postings.sort_by(|left, right| left.key.cmp(&right.key));
            let mut directory = Vec::with_capacity(postings.len());
            for posting in &postings {
                let offset = archive.position();
                write_fuzzy_posting(&mut archive, posting)?;
                let end = archive.position();
                directory.push(FuzzyDirectoryEntry {
                    key: posting.key.clone(),
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
            archive.write_all(FUZZY_INDEX_FOOTER)?;
        }
        let mut footer = Vec::new();
        write_checksum_footer(&mut footer, &bytes, FUZZY_CHECKSUM_FOOTER)?;
        bytes.extend(footer);
        writer.write_all(&bytes)?;
        Ok(())
    })
    .map(|_| ())
}

pub fn read_fuzzy_postings(path: impl AsRef<Path>) -> Result<Vec<FuzzyPosting>> {
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    let mut magic = vec![0; FUZZY_MAGIC_V1.len()];
    file.read_exact(&mut magic)
        .map_err(|err| GfmError::io(path, err))?;
    if magic != FUZZY_MAGIC_V1 {
        return Err(fuzzy_format_error(path, "unsupported fuzzy header"));
    }
    verify_fuzzy_checksum_for_file(&mut file, path)?;
    let count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
    let mut postings = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        postings.push(read_fuzzy_posting(&mut file, path)?);
    }
    Ok(postings)
}

impl MmapFuzzyArchive {
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
            // SAFETY: Fuzzy archives are immutable after atomic publication and
            // this reader only exposes bounds-checked immutable slices.
            unsafe { MmapOptions::new().map(&file) }.map_err(|err| GfmError::io(path, err))?
        };
        check_control()?;
        if mmap.get(..FUZZY_MAGIC_V1.len()) != Some(FUZZY_MAGIC_V1) {
            return Err(fuzzy_format_error(path, "unsupported fuzzy header"));
        }
        check_control()?;
        verify_fuzzy_checksum_from_slice(&mmap, path)?;
        check_control()?;
        let directory = read_fuzzy_directory_from_slice(&mmap, path)?;
        check_control()?;
        Ok(Self {
            path: path.to_path_buf(),
            mmap,
            directory,
        })
    }

    pub fn terms_for(&self, key: &str) -> Result<Vec<String>> {
        Ok(self
            .posting_for(key)?
            .map(|posting| posting.terms)
            .unwrap_or_default())
    }

    pub fn terms_for_limit(&self, key: &str, limit: usize) -> Result<(Vec<String>, bool)> {
        let key = normalize(key);
        if key.is_empty() || limit == 0 {
            return Ok((Vec::new(), false));
        }
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.key.as_str().cmp(key.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok((Vec::new(), false));
        };
        let posting = self.limited_posting_for_entry(entry, limit)?;
        Ok((posting.posting.terms, posting.truncated))
    }

    pub fn postings_for_sorted_keys_limit<I, S>(
        &self,
        keys: I,
        limit_per_key: usize,
    ) -> Result<Vec<LimitedFuzzyPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if limit_per_key == 0 {
            return Ok(Vec::new());
        }

        let mut postings = Vec::new();
        let mut directory_index = 0usize;
        let mut previous: Option<String> = None;

        for key in keys {
            let key = normalize(key.as_ref());
            if key.is_empty() {
                continue;
            }
            if let Some(previous_key) = previous.as_ref() {
                if key < *previous_key {
                    return Err(fuzzy_format_error(
                        &self.path,
                        "batch fuzzy lookup keys must be sorted",
                    ));
                }
                if key == *previous_key {
                    continue;
                }
            }

            while let Some(entry) = self.directory.get(directory_index) {
                if entry.key.as_str() >= key.as_str() {
                    break;
                }
                directory_index += 1;
            }

            if let Some(entry) = self.directory.get(directory_index) {
                if entry.key.as_str() == key.as_str() {
                    postings.push(self.limited_posting_for_entry(entry, limit_per_key)?);
                }
            }
            previous = Some(key);
        }

        Ok(postings)
    }

    pub fn postings(&self) -> Result<Vec<FuzzyPosting>> {
        self.directory
            .iter()
            .map(|entry| {
                let bytes = self.posting_bytes(entry)?;
                read_fuzzy_posting(Cursor::new(bytes), &self.path)
            })
            .collect()
    }

    pub fn postings_for<I, S>(&self, keys: I) -> Result<Vec<FuzzyPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();
        for key in keys {
            let key = normalize(key.as_ref());
            if !key.is_empty() {
                selected.insert(key);
            }
        }

        self.postings_for_sorted_keys_limit(selected, usize::MAX)
            .map(|postings| {
                postings
                    .into_iter()
                    .map(|posting| posting.posting)
                    .collect()
            })
    }

    pub fn posting_for(&self, key: &str) -> Result<Option<FuzzyPosting>> {
        let key = normalize(key);
        if key.is_empty() {
            return Ok(None);
        }
        let Some(entry) = self
            .directory
            .binary_search_by(|entry| entry.key.as_str().cmp(key.as_str()))
            .ok()
            .map(|index| &self.directory[index])
        else {
            return Ok(None);
        };
        let bytes = self.posting_bytes(entry)?;
        let posting = read_fuzzy_posting(Cursor::new(bytes), &self.path)?;
        if posting.key == key {
            Ok(Some(posting))
        } else {
            Err(fuzzy_format_error(
                &self.path,
                "fuzzy directory points at the wrong posting",
            ))
        }
    }

    pub fn indexed_keys(&self) -> usize {
        self.directory.len()
    }

    pub fn mapped_len(&self) -> usize {
        self.mmap.len()
    }

    pub fn is_checksummed(&self) -> bool {
        verify_checksum_footer(&self.mmap, FUZZY_CHECKSUM_FOOTER, &self.path, "fuzzy")
            .unwrap_or(false)
    }

    fn posting_bytes(&self, entry: &FuzzyDirectoryEntry) -> Result<&[u8]> {
        let start = usize::try_from(entry.offset)
            .map_err(|_| fuzzy_format_error(&self.path, "posting offset overflow"))?;
        let len = usize::try_from(entry.len)
            .map_err(|_| fuzzy_format_error(&self.path, "posting length overflow"))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| fuzzy_format_error(&self.path, "posting range overflow"))?;
        self.mmap
            .get(start..end)
            .ok_or_else(|| fuzzy_format_error(&self.path, "posting range out of bounds"))
    }

    fn limited_posting_for_entry(
        &self,
        entry: &FuzzyDirectoryEntry,
        limit: usize,
    ) -> Result<LimitedFuzzyPosting> {
        let bytes = self.posting_bytes(entry)?;
        let mut cursor = Cursor::new(bytes);
        let posting_key = read_string(&mut cursor, &self.path, "key")?;
        if posting_key != entry.key {
            return Err(fuzzy_format_error(
                &self.path,
                "fuzzy directory points at the wrong posting",
            ));
        }
        let count = read_varint(&mut cursor).map_err(|err| GfmError::io(&self.path, err))?;
        let capacity_count = usize::try_from(count).unwrap_or(usize::MAX);
        let mut terms = Vec::with_capacity(limit.min(capacity_count));
        let read_count = count.min(limit as u64);
        for _ in 0..read_count {
            terms.push(read_string(&mut cursor, &self.path, "term")?);
        }
        Ok(LimitedFuzzyPosting {
            posting: FuzzyPosting {
                key: posting_key,
                terms,
            },
            truncated: count > limit as u64,
        })
    }
}

fn write_fuzzy_posting(mut writer: impl Write, posting: &FuzzyPosting) -> std::io::Result<()> {
    let key = normalize(&posting.key);
    write_varint(&mut writer, key.len() as u64)?;
    writer.write_all(key.as_bytes())?;
    let mut terms = posting
        .terms
        .iter()
        .map(|term| normalize(term))
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    terms.sort();
    terms.dedup();
    write_varint(&mut writer, terms.len() as u64)?;
    for term in terms {
        write_varint(&mut writer, term.len() as u64)?;
        writer.write_all(term.as_bytes())?;
    }
    Ok(())
}

fn read_fuzzy_posting(mut reader: impl Read, path: &Path) -> Result<FuzzyPosting> {
    let key = read_string(&mut reader, path, "key")?;
    let count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut terms = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        terms.push(read_string(&mut reader, path, "term")?);
    }
    Ok(FuzzyPosting { key, terms })
}

fn read_string(mut reader: impl Read, path: &Path, label: &str) -> Result<String> {
    let len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let len = usize::try_from(len)
        .map_err(|_| fuzzy_format_error(path, &format!("{label} length overflow")))?;
    let mut value = vec![0; len];
    reader
        .read_exact(&mut value)
        .map_err(|err| GfmError::io(path, err))?;
    String::from_utf8(value)
        .map_err(|err| fuzzy_format_error(path, &format!("invalid UTF-8 {label}: {err}")))
}

fn write_directory_entry(
    mut writer: impl Write,
    entry: &FuzzyDirectoryEntry,
) -> std::io::Result<()> {
    write_varint(&mut writer, entry.key.len() as u64)?;
    writer.write_all(entry.key.as_bytes())?;
    writer.write_all(&entry.offset.to_le_bytes())?;
    writer.write_all(&entry.len.to_le_bytes())
}

fn read_fuzzy_directory_from_slice(bytes: &[u8], path: &Path) -> Result<Vec<FuzzyDirectoryEntry>> {
    let indexed_len = fuzzy_indexed_len_from_slice(bytes, path)?;
    let footer_start = indexed_len
        .checked_sub(FUZZY_INDEX_FOOTER.len())
        .and_then(|value| value.checked_sub(8))
        .ok_or_else(|| fuzzy_format_error(path, "missing fuzzy directory footer"))?;
    let mut directory_offset = [0u8; 8];
    directory_offset.copy_from_slice(
        bytes
            .get(footer_start..footer_start + 8)
            .ok_or_else(|| fuzzy_format_error(path, "missing fuzzy directory footer"))?,
    );
    let directory_offset = usize::try_from(u64::from_le_bytes(directory_offset))
        .map_err(|_| fuzzy_format_error(path, "invalid fuzzy directory offset"))?;
    if directory_offset >= footer_start {
        return Err(fuzzy_format_error(path, "invalid fuzzy directory offset"));
    }
    let mut reader = Cursor::new(
        bytes
            .get(directory_offset..footer_start)
            .ok_or_else(|| fuzzy_format_error(path, "fuzzy directory out of bounds"))?,
    );
    let count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut directory = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        let key = read_string(&mut reader, path, "directory key")?;
        let mut offset = [0u8; 8];
        reader
            .read_exact(&mut offset)
            .map_err(|err| GfmError::io(path, err))?;
        let mut len = [0u8; 8];
        reader
            .read_exact(&mut len)
            .map_err(|err| GfmError::io(path, err))?;
        directory.push(FuzzyDirectoryEntry {
            key,
            offset: u64::from_le_bytes(offset),
            len: u64::from_le_bytes(len),
        });
    }
    directory.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(directory)
}

fn verify_fuzzy_checksum_for_file(file: &mut File, path: &Path) -> Result<()> {
    let mut full = Vec::new();
    file.rewind().map_err(|err| GfmError::io(path, err))?;
    file.read_to_end(&mut full)
        .map_err(|err| GfmError::io(path, err))?;
    verify_fuzzy_checksum_from_slice(&full, path)
}

fn verify_fuzzy_checksum_from_slice(bytes: &[u8], path: &Path) -> Result<()> {
    if !verify_checksum_footer(bytes, FUZZY_CHECKSUM_FOOTER, path, "fuzzy")? {
        return Err(fuzzy_format_error(path, "missing fuzzy checksum footer"));
    }
    Ok(())
}

fn fuzzy_indexed_len_from_slice(bytes: &[u8], path: &Path) -> Result<usize> {
    let footer_len = 4usize
        .checked_add(FUZZY_CHECKSUM_FOOTER.len())
        .ok_or_else(|| fuzzy_format_error(path, "fuzzy checksum footer length overflow"))?;
    if bytes.len() < footer_len {
        return Err(fuzzy_format_error(path, "missing fuzzy checksum footer"));
    }
    let indexed_len = bytes.len() - footer_len;
    if bytes.get(indexed_len + 4..) != Some(FUZZY_CHECKSUM_FOOTER) {
        return Err(fuzzy_format_error(path, "missing fuzzy checksum footer"));
    }
    Ok(indexed_len)
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn is_fuzzy_term(term: &str) -> bool {
    let mut count = 0;
    let mut has_alpha = false;
    let mut consecutive_digits = 0;
    for ch in term.chars() {
        count += 1;
        if ch.is_alphabetic() {
            has_alpha = true;
            consecutive_digits = 0;
        } else if ch.is_ascii_digit() {
            consecutive_digits += 1;
            if consecutive_digits > 4 {
                return false;
            }
        } else {
            consecutive_digits = 0;
        }
    }
    (FUZZY_MIN_TERM_LEN..=FUZZY_MAX_TERM_LEN).contains(&count) && has_alpha
}

fn deletion_keys(term: &str, max_deletions: usize) -> Vec<String> {
    let chars: Vec<char> = term.chars().collect();
    if !is_fuzzy_term(term) {
        return Vec::new();
    }
    let mut keys = BTreeSet::new();
    collect_deletions(&chars, max_deletions, &mut keys);
    keys.into_iter().collect()
}

fn collect_deletions(chars: &[char], remaining: usize, keys: &mut BTreeSet<String>) {
    keys.insert(chars.iter().collect());
    if remaining == 0 || chars.len() <= 1 {
        return;
    }
    for index in 0..chars.len() {
        let mut next = Vec::with_capacity(chars.len() - 1);
        next.extend_from_slice(&chars[..index]);
        next.extend_from_slice(&chars[index + 1..]);
        collect_deletions(&next, remaining - 1, keys);
    }
}

fn fuzzy_format_error(path: &Path, reason: &str) -> GfmError {
    GfmError::Format(format!("invalid fuzzy store {}: {reason}", path.display()))
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
    use gfm_types::{FileId, FileKind, VolumeId};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn mmap_fuzzy_archive_reads_candidate_terms() {
        let path = temp_path("gfm-fuzzy-mmap", "gfmfuzzy");
        let posting = FuzzyPosting {
            key: "plan".to_string(),
            terms: vec!["plane".to_string(), "plans".to_string()],
        };

        write_fuzzy_postings(&path, std::slice::from_ref(&posting)).unwrap();
        let archive = MmapFuzzyArchive::open(&path).unwrap();

        assert_eq!(
            archive.terms_for("PLAN").unwrap(),
            vec!["plane".to_string(), "plans".to_string()]
        );
        assert_eq!(
            archive.terms_for_limit("PLAN", 1).unwrap(),
            (vec!["plane".to_string()], true)
        );
        assert_eq!(
            archive.terms_for_limit("plan", 4).unwrap(),
            (vec!["plane".to_string(), "plans".to_string()], false)
        );
        assert_eq!(
            archive.postings_for(["missing", "PLAN", "plan"]).unwrap(),
            vec![posting]
        );
        assert!(archive.is_checksummed());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_fuzzy_archive_reads_bounded_sorted_postings_in_one_pass() {
        let path = temp_path("gfm-fuzzy-batch-postings", "gfmfuzzy");
        let postings = vec![
            FuzzyPosting {
                key: "aplha".to_string(),
                terms: vec![
                    "alpha".to_string(),
                    "alphanum".to_string(),
                    "alphas".to_string(),
                ],
            },
            FuzzyPosting {
                key: "projet".to_string(),
                terms: vec![
                    "project".to_string(),
                    "projected".to_string(),
                    "projects".to_string(),
                    "projectx".to_string(),
                ],
            },
        ];

        write_fuzzy_postings(&path, &postings).unwrap();
        let archive = MmapFuzzyArchive::open(&path).unwrap();
        let batch = archive
            .postings_for_sorted_keys_limit(["", "aplha", "aplha", "missing", "projet"], 2)
            .unwrap();

        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].posting.key, "aplha");
        assert_eq!(batch[0].posting.terms, postings[0].terms[..2]);
        assert!(batch[0].truncated);
        assert_eq!(batch[1].posting.key, "projet");
        assert_eq!(batch[1].posting.terms, postings[1].terms[..2]);
        assert!(batch[1].truncated);
        assert!(archive
            .postings_for_sorted_keys_limit(["projet", "aplha"], 2)
            .is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn fuzzy_postings_from_records_include_deletion_keys() {
        let postings = fuzzy_postings_from_records(&[record(1, "/tmp/tagged.md", "tagged.md")]);
        let tagge = postings
            .iter()
            .find(|posting| posting.key == "tagge")
            .unwrap();
        let md = postings.iter().find(|posting| posting.key == "md").unwrap();

        assert_eq!(tagge.terms, vec!["tagged"]);
        assert_eq!(md.terms, vec!["md"]);
    }

    #[test]
    fn fuzzy_postings_skip_numeric_only_and_digit_run_terms() {
        let postings = fuzzy_postings_from_records(&[record(
            1,
            "/tmp/project-PackageProject00012345.md",
            "project-PackageProject00012345.md",
        )]);

        assert!(postings
            .iter()
            .any(|posting| posting.terms == vec!["project".to_string()]));
        assert!(!postings
            .iter()
            .any(|posting| posting.terms.iter().any(|term| term == "00012345")));
        assert!(!postings.iter().any(|posting| posting
            .terms
            .iter()
            .any(|term| term == "packageproject00012345")));
    }

    #[test]
    fn mmap_fuzzy_archive_checked_open_honors_pre_cancelled_control_before_file_open() {
        let path = temp_path("gfm-fuzzy-open-cancel", "gfmfuzzy");

        let result = MmapFuzzyArchive::open_checked(&path, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!path.exists());
    }

    #[test]
    fn checksummed_fuzzy_archive_rejects_corruption() {
        let path = temp_path("gfm-fuzzy-checksum", "gfmfuzzy");
        write_fuzzy_postings(
            &path,
            &[FuzzyPosting {
                key: "tagge".to_string(),
                terms: vec!["tagged".to_string()],
            }],
        )
        .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let index = FUZZY_MAGIC_V1.len() + 2;
        bytes[index] ^= 0x10;
        std::fs::write(&path, bytes).unwrap();

        let read_error = read_fuzzy_postings(&path).unwrap_err().to_string();
        let mmap_error = MmapFuzzyArchive::open(&path).unwrap_err().to_string();

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
