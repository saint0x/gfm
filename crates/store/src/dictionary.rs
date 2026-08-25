use crate::durable;
use gfm_types::{FileKind, FileRecord, GfmError, Result};
use memmap2::{Mmap, MmapOptions};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

const DICTIONARY_MAGIC_V1: &[u8] = b"gfm-dictionary-v1\n";
const DICTIONARY_INDEX_FOOTER: &[u8] = b"gfm-dictionary-index-v1\n";
const DICTIONARY_FOOTER_LEN: u64 = 8 + DICTIONARY_INDEX_FOOTER.len() as u64;
const DEFAULT_BLOCK_SIZE: usize = 16;

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
    let mut terms = BTreeSet::new();
    terms.insert("field:comment".to_string());
    terms.insert("field:extension".to_string());
    terms.insert("field:kind".to_string());
    terms.insert("field:path".to_string());
    terms.insert("field:tag".to_string());
    for record in records {
        insert_term(&mut terms, record.name.as_str());
        insert_term(&mut terms, kind_term(record.kind));
        insert_term(&mut terms, &record.path.to_string_lossy());
        for component in record.path.components() {
            if let Some(component) = component.as_os_str().to_str() {
                insert_term(&mut terms, component);
            }
        }
        if let Some(parent) = record.path.parent() {
            insert_term(&mut terms, &parent.to_string_lossy());
        }
        if let Some(extension) = record.extension() {
            insert_term(&mut terms, extension);
        }
        for tag in &record.tags {
            insert_term(&mut terms, tag);
        }
        if let Some(comment) = &record.finder_comment {
            for token in tokenize(&normalize(comment)) {
                insert_term(&mut terms, &token);
            }
        }
    }
    terms.into_iter().collect()
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
        let mut writer = CountingWriter::new(writer);
        writer.write_all(DICTIONARY_MAGIC_V1)?;
        write_varint(&mut writer, terms.len() as u64)?;
        write_varint(&mut writer, block_size as u64)?;
        let mut blocks = Vec::new();
        let mut previous = String::new();
        for (index, term) in terms.iter().enumerate() {
            if index % block_size == 0 {
                let offset = writer.position();
                blocks.push(DictionaryBlock {
                    first: term.clone(),
                    offset,
                    entries: 0,
                });
                previous.clear();
            }
            write_front_coded(&mut writer, &previous, term)?;
            previous = term.clone();
            if let Some(block) = blocks.last_mut() {
                block.entries += 1;
            }
        }
        let directory_offset = writer.position();
        write_varint(&mut writer, blocks.len() as u64)?;
        for block in &blocks {
            write_varint(&mut writer, block.first.len() as u64)?;
            writer.write_all(block.first.as_bytes())?;
            write_varint(&mut writer, block.offset)?;
            write_varint(&mut writer, block.entries)?;
        }
        writer.write_all(&directory_offset.to_le_bytes())?;
        writer.write_all(DICTIONARY_INDEX_FOOTER)?;
        Ok(())
    })
    .map(|_| ())
}

impl MmapDictionary {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mmap = {
            // SAFETY: The dictionary is mapped read-only and accessed only through
            // checked immutable slices. Writers publish complete files with atomic
            // rename, so this API never mutates or aliases writable mapped bytes.
            unsafe { MmapOptions::new().map(&file) }.map_err(|err| GfmError::io(path, err))?
        };
        if !mmap.starts_with(DICTIONARY_MAGIC_V1) {
            return Err(dictionary_format_error(
                path,
                "unsupported dictionary header",
            ));
        }
        let mut cursor = Cursor::new(&mmap[DICTIONARY_MAGIC_V1.len()..]);
        let len = usize::try_from(read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?)
            .map_err(|_| dictionary_format_error(path, "dictionary length overflow"))?;
        let block_size =
            usize::try_from(read_varint(&mut cursor).map_err(|err| GfmError::io(path, err))?)
                .map_err(|_| dictionary_format_error(path, "dictionary block size overflow"))?;
        let blocks = read_dictionary_directory_from_slice(&mmap, path)?;
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
            .unwrap_or_else(|| dictionary_directory_offset(&self.mmap).unwrap_or(self.mmap.len()));
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
    let footer_offset = bytes
        .len()
        .checked_sub(DICTIONARY_FOOTER_LEN as usize)
        .ok_or_else(|| dictionary_format_error(path, "missing dictionary directory footer"))?;
    if directory_offset >= footer_offset {
        return Err(dictionary_format_error(
            path,
            "invalid dictionary directory offset",
        ));
    }
    let footer = bytes
        .get(footer_offset + 8..)
        .ok_or_else(|| dictionary_format_error(path, "missing dictionary directory footer"))?;
    if footer != DICTIONARY_INDEX_FOOTER {
        return Err(dictionary_format_error(
            path,
            "missing dictionary directory footer",
        ));
    }
    let mut cursor = Cursor::new(&bytes[directory_offset..footer_offset]);
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
    if bytes.len() < DICTIONARY_FOOTER_LEN as usize {
        return None;
    }
    let footer_offset = bytes.len() - DICTIONARY_FOOTER_LEN as usize;
    let mut offset = [0u8; 8];
    offset.copy_from_slice(bytes.get(footer_offset..footer_offset + 8)?);
    usize::try_from(u64::from_le_bytes(offset)).ok()
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
    fn dictionary_terms_include_record_paths_tags_kinds_and_metadata_keys() {
        let record = FileRecord {
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

        let terms = dictionary_terms_from_records(&[record]);

        assert!(terms.contains(&"field:tag".to_string()));
        assert!(terms.contains(&"kind:file".to_string()));
        assert!(terms.contains(&"important".to_string()));
        assert!(terms.contains(&"md".to_string()));
        assert!(terms.contains(&"client".to_string()));
        assert!(terms.contains(&"/users/deepsaint/documents/report.md".to_string()));
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
