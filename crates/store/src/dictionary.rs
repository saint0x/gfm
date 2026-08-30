use crate::durable;
use crate::integrity::{verify_checksum_footer, write_checksum_footer};
use gfm_types::{FileKind, FileRecord, GfmError, Result};
use memmap2::{Mmap, MmapOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

const DICTIONARY_MAGIC_V1: &[u8] = b"gfm-dictionary-v1\n";
const DICTIONARY_INDEX_FOOTER: &[u8] = b"gfm-dictionary-index-v1\n";
const DICTIONARY_CHECKSUM_FOOTER: &[u8] = b"gfm-dictionary-checksum-v1\n";
const DICTIONARY_FOOTER_LEN: u64 = 8 + DICTIONARY_INDEX_FOOTER.len() as u64;
const DEFAULT_BLOCK_SIZE: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictionaryTermReport {
    pub terms: Vec<String>,
    pub paths: usize,
    pub path_prefixes: usize,
    pub extensions: usize,
    pub tags: usize,
    pub kinds: usize,
    pub metadata_keys: usize,
    pub comment_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DictionaryBlock {
    first: String,
    offset: u64,
    entries: u64,
}

#[derive(Debug)]
pub struct MmapDictionary {
    path: PathBuf,
    mmap: Mmap,
    blocks: Vec<DictionaryBlock>,
    len: usize,
    block_size: usize,
}

pub fn dictionary_terms_from_records(records: &[FileRecord]) -> Vec<String> {
    dictionary_term_report_from_records(records).terms
}

pub fn dictionary_term_report_from_records(records: &[FileRecord]) -> DictionaryTermReport {
    let mut terms = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut path_prefix_counts = BTreeMap::new();
    let mut path_prefixes = BTreeSet::new();
    let mut extensions = BTreeSet::new();
    let mut tags = BTreeSet::new();
    let mut kinds = BTreeSet::new();
    let mut metadata_keys = BTreeSet::new();
    let mut comment_tokens = BTreeSet::new();

    for key in [
        "field:comment",
        "field:extension",
        "field:kind",
        "field:path",
        "field:path-prefix",
        "field:tag",
    ] {
        insert_classified_term(&mut terms, &mut metadata_keys, key);
    }

    for record in records {
        insert_term(&mut terms, record.name.as_str());
        insert_classified_term(&mut terms, &mut kinds, kind_term(record.kind));
        insert_classified_term(&mut terms, &mut paths, &record.path.to_string_lossy());
        for component in record.path.components() {
            if let Some(component) = component.as_os_str().to_str() {
                insert_term(&mut terms, component);
            }
        }
        if let Some(parent) = record.path.parent() {
            insert_classified_term(&mut terms, &mut paths, &parent.to_string_lossy());
            let mut record_prefixes = BTreeSet::new();
            for ancestor in parent.ancestors() {
                let prefix = normalize_path_prefix(ancestor);
                if !prefix.is_empty() {
                    record_prefixes.insert(prefix);
                }
            }
            for prefix in record_prefixes {
                *path_prefix_counts.entry(prefix).or_insert(0usize) += 1;
            }
        }
        if let Some(extension) = record.extension() {
            insert_classified_term(&mut terms, &mut extensions, extension);
        }
        for tag in &record.tags {
            insert_classified_term(&mut terms, &mut tags, tag);
        }
        if let Some(comment) = &record.finder_comment {
            for token in tokenize(&normalize(comment)) {
                insert_classified_term(&mut terms, &mut comment_tokens, &token);
            }
        }
    }

    for (prefix, count) in path_prefix_counts {
        if count > 1 {
            paths.insert(prefix.clone());
            path_prefixes.insert(prefix.clone());
            terms.insert(prefix);
        }
    }

    DictionaryTermReport {
        terms: terms.into_iter().collect(),
        paths: paths.len(),
        path_prefixes: path_prefixes.len(),
        extensions: extensions.len(),
        tags: tags.len(),
        kinds: kinds.len(),
        metadata_keys: metadata_keys.len(),
        comment_tokens: comment_tokens.len(),
    }
}

pub fn write_dictionary(path: impl AsRef<Path>, terms: &[String]) -> Result<()> {
    write_dictionary_with_block_size(path, terms, DEFAULT_BLOCK_SIZE)
}

pub fn read_dictionary(path: impl AsRef<Path>) -> Result<Vec<String>> {
    let archive = MmapDictionary::open(path)?;
    (0..archive.len())
        .map(|index| archive.get(index))
        .collect::<Result<Vec<_>>>()
}

fn write_dictionary_with_block_size(
    path: impl AsRef<Path>,
    terms: &[String],
    block_size: usize,
) -> Result<()> {
    let path = path.as_ref();
    let block_size = block_size.max(1);
    let mut terms: Vec<_> = terms
        .iter()
        .map(|term| normalize(term))
        .filter(|term| !term.is_empty())
        .collect();
    terms.sort();
    terms.dedup();
    durable::atomic_write(path, |writer| {
        let mut bytes = Vec::new();
        {
            let mut archive = CountingWriter::new(&mut bytes);
            archive.write_all(DICTIONARY_MAGIC_V1)?;
            write_varint(&mut archive, terms.len() as u64)?;
            write_varint(&mut archive, block_size as u64)?;
            let mut blocks = Vec::new();
            let mut previous = String::new();
            for (index, term) in terms.iter().enumerate() {
                if index % block_size == 0 {
                    let offset = archive.position();
                    blocks.push(DictionaryBlock {
                        first: term.clone(),
                        offset,
                        entries: 0,
                    });
                    previous.clear();
                }
                write_front_coded(&mut archive, &previous, term)?;
                previous = term.clone();
                if let Some(block) = blocks.last_mut() {
                    block.entries += 1;
                }
            }
            let directory_offset = archive.position();
            write_varint(&mut archive, blocks.len() as u64)?;
            for block in &blocks {
                write_varint(&mut archive, block.first.len() as u64)?;
                archive.write_all(block.first.as_bytes())?;
                write_varint(&mut archive, block.offset)?;
                write_varint(&mut archive, block.entries)?;
            }
            archive.write_all(&directory_offset.to_le_bytes())?;
            archive.write_all(DICTIONARY_INDEX_FOOTER)?;
        }
        let mut footer = Vec::new();
        write_checksum_footer(&mut footer, &bytes, DICTIONARY_CHECKSUM_FOOTER)?;
        bytes.extend(footer);
        writer.write_all(&bytes)?;
        Ok(())
    })
    .map(|_| ())
}

impl MmapDictionary {
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
            // SAFETY: The dictionary is mapped read-only and accessed only through
            // checked immutable slices. Writers publish complete files with atomic
            // rename, so this API never mutates or aliases writable mapped bytes.
            unsafe { MmapOptions::new().map(&file) }.map_err(|err| GfmError::io(path, err))?
        };
        check_control()?;
        if !mmap.starts_with(DICTIONARY_MAGIC_V1) {
            return Err(dictionary_format_error(
                path,
                "unsupported dictionary header",
            ));
        }
        check_control()?;
        verify_dictionary_checksum_from_slice(&mmap, path)?;
        check_control()?;
        let mut cursor = Cursor::new(&mmap[DICTIONARY_MAGIC_V1.len()..]);
        let len = usize::try_from(read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?)
            .map_err(|_| dictionary_format_error(path, "dictionary length overflow"))?;
        check_control()?;
        let block_size =
            usize::try_from(read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?)
                .map_err(|_| dictionary_format_error(path, "dictionary block size overflow"))?;
        check_control()?;
        let blocks = read_dictionary_directory_from_slice(&mmap, path)?;
        check_control()?;
        Ok(Self {
            path: path.to_path_buf(),
            mmap,
            blocks,
            len,
            block_size,
        })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn mapped_len(&self) -> usize {
        self.mmap.len()
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn is_checksummed(&self) -> bool {
        has_dictionary_checksum_footer(&self.mmap)
    }

    pub fn contains(&self, term: &str) -> Result<bool> {
        Ok(self.find(term)?.is_some())
    }

    pub fn find(&self, term: &str) -> Result<Option<usize>> {
        let term = normalize(term);
        if term.is_empty() || self.blocks.is_empty() {
            return Ok(None);
        }
        let block_index = match self
            .blocks
            .binary_search_by(|block| block.first.as_str().cmp(term.as_str()))
        {
            Ok(index) => index,
            Err(0) => return Ok(None),
            Err(index) => index - 1,
        };
        let entries = self.decode_block(block_index)?;
        for (entry_index, value) in entries.into_iter().enumerate() {
            match value.as_str().cmp(term.as_str()) {
                std::cmp::Ordering::Equal => {
                    return Ok(Some(block_index * self.block_size + entry_index));
                }
                std::cmp::Ordering::Greater => return Ok(None),
                std::cmp::Ordering::Less => {}
            }
        }
        Ok(None)
    }

    pub fn get(&self, index: usize) -> Result<String> {
        if index >= self.len {
            return Err(dictionary_format_error(
                &self.path,
                "dictionary index out of bounds",
            ));
        }
        let block_index = index / self.block_size;
        let entry_index = index % self.block_size;
        self.decode_block(block_index)?
            .get(entry_index)
            .cloned()
            .ok_or_else(|| dictionary_format_error(&self.path, "dictionary block truncated"))
    }

    fn decode_block(&self, block_index: usize) -> Result<Vec<String>> {
        let block = self
            .blocks
            .get(block_index)
            .ok_or_else(|| dictionary_format_error(&self.path, "dictionary block missing"))?;
        let start = usize::try_from(block.offset)
            .map_err(|_| dictionary_format_error(&self.path, "dictionary block offset overflow"))?;
        let end = self
            .blocks
            .get(block_index + 1)
            .and_then(|next| usize::try_from(next.offset).ok())
            .unwrap_or_else(|| {
                dictionary_directory_offset(&self.mmap).unwrap_or_else(|| {
                    dictionary_indexed_len_from_slice(&self.mmap, &self.path)
                        .unwrap_or(self.mmap.len())
                })
            });
        let bytes = self
            .mmap
            .get(start..end)
            .ok_or_else(|| dictionary_format_error(&self.path, "dictionary block out of bounds"))?;
        let mut cursor = Cursor::new(bytes);
        let mut previous = String::new();
        let mut entries = Vec::with_capacity(block.entries.min(1_000_000) as usize);
        for _ in 0..block.entries {
            let value = read_front_coded(&mut cursor, &previous, &self.path)?;
            previous = value.clone();
            entries.push(value);
        }
        Ok(entries)
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

fn write_front_coded(mut writer: impl Write, previous: &str, value: &str) -> std::io::Result<()> {
    let prefix = common_prefix_len(previous.as_bytes(), value.as_bytes());
    let suffix = &value.as_bytes()[prefix..];
    write_varint(&mut writer, prefix as u64)?;
    write_varint(&mut writer, suffix.len() as u64)?;
    writer.write_all(suffix)
}

fn read_front_coded(mut reader: impl Read, previous: &str, path: &Path) -> Result<String> {
    let prefix = usize::try_from(read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?)
        .map_err(|_| dictionary_format_error(path, "dictionary prefix overflow"))?;
    if prefix > previous.len() || !previous.is_char_boundary(prefix) {
        return Err(dictionary_format_error(path, "invalid dictionary prefix"));
    }
    let suffix_len =
        usize::try_from(read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?)
            .map_err(|_| dictionary_format_error(path, "dictionary suffix overflow"))?;
    let mut suffix = vec![0; suffix_len];
    reader
        .read_exact(&mut suffix)
        .map_err(|err| GfmError::io(path, err))?;
    let suffix = std::str::from_utf8(&suffix).map_err(|err| {
        GfmError::Format(format!(
            "invalid UTF-8 dictionary suffix in {}: {err}",
            path.display()
        ))
    })?;
    Ok(format!("{}{}", &previous[..prefix], suffix))
}

fn read_dictionary_directory_from_slice(bytes: &[u8], path: &Path) -> Result<Vec<DictionaryBlock>> {
    let directory_offset = dictionary_directory_offset(bytes)
        .ok_or_else(|| dictionary_format_error(path, "missing dictionary directory footer"))?;
    let indexed_len = dictionary_indexed_len_from_slice(bytes, path)?;
    let archive_bytes = bytes
        .get(..indexed_len)
        .ok_or_else(|| dictionary_format_error(path, "dictionary indexed range out of bounds"))?;
    let footer_offset = archive_bytes
        .len()
        .checked_sub(DICTIONARY_FOOTER_LEN as usize)
        .ok_or_else(|| dictionary_format_error(path, "missing dictionary directory footer"))?;
    if directory_offset >= footer_offset {
        return Err(dictionary_format_error(
            path,
            "invalid dictionary directory offset",
        ));
    }
    let footer = archive_bytes
        .get(footer_offset + 8..)
        .ok_or_else(|| dictionary_format_error(path, "missing dictionary directory footer"))?;
    if footer != DICTIONARY_INDEX_FOOTER {
        return Err(dictionary_format_error(
            path,
            "missing dictionary directory footer",
        ));
    }
    let mut cursor = Cursor::new(&archive_bytes[directory_offset..footer_offset]);
    let count = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
    let mut blocks = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        let term_len = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
        let mut term = vec![0; term_len as usize];
        cursor
            .read_exact(&mut term)
            .map_err(|err| GfmError::io(path, err))?;
        let first = String::from_utf8(term).map_err(|err| {
            GfmError::Format(format!(
                "invalid UTF-8 dictionary block term in {}: {err}",
                path.display()
            ))
        })?;
        let offset = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
        let entries = read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?;
        blocks.push(DictionaryBlock {
            first,
            offset,
            entries,
        });
    }
    Ok(blocks)
}

fn dictionary_directory_offset(bytes: &[u8]) -> Option<usize> {
    let indexed_len = dictionary_indexed_len_from_slice(bytes, Path::new("<dictionary>")).ok()?;
    if indexed_len < DICTIONARY_FOOTER_LEN as usize {
        return None;
    }
    let footer_offset = indexed_len - DICTIONARY_FOOTER_LEN as usize;
    let mut offset = [0u8; 8];
    offset.copy_from_slice(bytes.get(footer_offset..footer_offset + 8)?);
    usize::try_from(u64::from_le_bytes(offset)).ok()
}

fn verify_dictionary_checksum_from_slice(bytes: &[u8], path: &Path) -> Result<()> {
    if has_dictionary_checksum_footer(bytes)
        && !verify_checksum_footer(bytes, DICTIONARY_CHECKSUM_FOOTER, path, "dictionary")?
    {
        return Err(dictionary_format_error(
            path,
            "missing dictionary checksum footer",
        ));
    }
    Ok(())
}

fn dictionary_indexed_len_from_slice(bytes: &[u8], path: &Path) -> Result<usize> {
    let footer_len = dictionary_checksum_footer_len();
    if bytes.len() < footer_len {
        return Ok(bytes.len());
    }
    let footer_start = bytes.len() - footer_len;
    if bytes.get(footer_start + 4..) == Some(DICTIONARY_CHECKSUM_FOOTER) {
        Ok(footer_start)
    } else {
        if bytes.starts_with(DICTIONARY_MAGIC_V1)
            && bytes
                .windows(DICTIONARY_CHECKSUM_FOOTER.len())
                .any(|window| window == DICTIONARY_CHECKSUM_FOOTER)
        {
            return Err(dictionary_format_error(
                path,
                "invalid dictionary checksum footer",
            ));
        }
        Ok(bytes.len())
    }
}

fn has_dictionary_checksum_footer(bytes: &[u8]) -> bool {
    let footer_len = dictionary_checksum_footer_len();
    bytes.len() >= footer_len
        && bytes.get(bytes.len() - footer_len + 4..) == Some(DICTIONARY_CHECKSUM_FOOTER)
}

const fn dictionary_checksum_footer_len() -> usize {
    4 + DICTIONARY_CHECKSUM_FOOTER.len()
}

fn common_prefix_len(left: &[u8], right: &[u8]) -> usize {
    let left = std::str::from_utf8(left).unwrap_or_default();
    let right = std::str::from_utf8(right).unwrap_or_default();
    let mut len = 0;
    for (left_ch, right_ch) in left.chars().zip(right.chars()) {
        if left_ch != right_ch {
            break;
        }
        len += left_ch.len_utf8();
    }
    len
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

fn insert_term(terms: &mut BTreeSet<String>, value: &str) {
    let value = normalize(value);
    if !value.is_empty() {
        terms.insert(value);
    }
}

fn insert_classified_term(
    terms: &mut BTreeSet<String>,
    classified: &mut BTreeSet<String>,
    value: &str,
) {
    let value = normalize(value);
    if !value.is_empty() {
        classified.insert(value.clone());
        terms.insert(value);
    }
}

fn normalize_path_prefix(path: &Path) -> String {
    let value = normalize(&path.to_string_lossy());
    if value == "/" || value == "." {
        String::new()
    } else {
        value
    }
}

fn kind_term(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "kind:directory",
        FileKind::File => "kind:file",
        FileKind::Symlink => "kind:symlink",
        FileKind::Other => "kind:other",
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

fn dictionary_format_error(path: &Path, reason: &str) -> GfmError {
    GfmError::Format(format!(
        "invalid dictionary store {}: {reason}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::{FileId, VolumeId};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn mmap_dictionary_reads_front_coded_terms() {
        let path = temp_path("gfm-dictionary-mmap", "gfmdict");
        let terms = vec![
            "/users/deepsaint/documents/report.md".to_string(),
            "/users/deepsaint/documents/report-final.md".to_string(),
            "/users/deepsaint/downloads/report.md".to_string(),
            "field:tag".to_string(),
            "important".to_string(),
            "kind:file".to_string(),
            "report".to_string(),
        ];

        write_dictionary_with_block_size(&path, &terms, 2).unwrap();
        let archive = MmapDictionary::open(&path).unwrap();
        let read = read_dictionary(&path).unwrap();

        assert_eq!(read.len(), terms.len());
        assert_eq!(archive.block_size(), 2);
        assert!(archive.mapped_len() > 0);
        assert!(archive.contains("IMPORTANT").unwrap());
        assert!(archive
            .find("/users/deepsaint/documents/report-final.md")
            .unwrap()
            .is_some());
        assert!(!archive.contains("missing").unwrap());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn checksummed_dictionary_rejects_corruption() {
        let path = temp_path("gfm-dictionary-checksum", "gfmdict");
        let terms = vec![
            "alpha".to_string(),
            "important".to_string(),
            "kind:file".to_string(),
        ];

        write_dictionary(&path, &terms).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let offset = bytes
            .windows(b"important".len())
            .position(|window| window == b"important")
            .expect("archive should contain the test term");
        bytes[offset] = b'z';
        std::fs::write(&path, bytes).unwrap();

        let error = MmapDictionary::open(&path).unwrap_err().to_string();

        assert!(error.contains("checksum mismatch"), "{error}");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mmap_dictionary_checked_open_honors_pre_cancelled_control_before_file_open() {
        let path = temp_path("gfm-dictionary-open-cancel", "gfmdict");

        let result = MmapDictionary::open_checked(&path, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!path.exists());
    }

    #[test]
    fn dictionary_terms_include_record_paths_tags_kinds_metadata_keys_and_shared_prefixes() {
        let first = FileRecord {
            id: FileId::new(VolumeId(4), 12),
            parent: None,
            path: PathBuf::from("/Users/deepsaint/Documents/Report.md"),
            name: "Report.md".to_string(),
            kind: FileKind::File,
            len: 1,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: vec!["Important".to_string()],
            finder_comment: Some("Client handoff".to_string()),
        };
        let second = FileRecord {
            id: FileId::new(VolumeId(4), 13),
            parent: None,
            path: PathBuf::from("/Users/deepsaint/Documents/Project/Notes.txt"),
            name: "Notes.txt".to_string(),
            kind: FileKind::File,
            len: 1,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: vec!["Client".to_string()],
            finder_comment: None,
        };

        let report = dictionary_term_report_from_records(&[first, second]);
        let terms = report.terms;

        assert!(terms.contains(&"field:tag".to_string()));
        assert!(terms.contains(&"field:path-prefix".to_string()));
        assert!(terms.contains(&"kind:file".to_string()));
        assert!(terms.contains(&"important".to_string()));
        assert!(terms.contains(&"md".to_string()));
        assert!(terms.contains(&"txt".to_string()));
        assert!(terms.contains(&"client".to_string()));
        assert!(terms.contains(&"/users/deepsaint/documents/report.md".to_string()));
        assert!(terms.contains(&"/users/deepsaint/documents".to_string()));
        assert!(terms.contains(&"/users/deepsaint".to_string()));
        assert_eq!(report.path_prefixes, 3);
        assert_eq!(report.extensions, 2);
        assert_eq!(report.tags, 2);
        assert_eq!(report.kinds, 1);
        assert_eq!(report.metadata_keys, 6);
        assert_eq!(report.comment_tokens, 2);
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
