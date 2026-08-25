use gfm_types::{
    ContentPositions, ContentPosting, ContentSegment, FileId, GfmError, Result, VolumeId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const CONTENT_MAGIC_V1: &[u8] = b"gfm-content-v1\n";
const CONTENT_MAGIC_V2: &[u8] = b"gfm-content-v2\n";
const CONTENT_MAGIC_V3: &[u8] = b"gfm-content-v3\n";
const CONTENT_SEGMENT_MAGIC: &[u8] = b"gfm-content-segment-v1\n";
const CONTENT_SEGMENT_MAGIC_V2: &[u8] = b"gfm-content-segment-v2\n";
const CONTENT_INDEX_FOOTER: &[u8] = b"gfm-content-index-v1\n";
const CONTENT_FOOTER_LEN: u64 = 8 + CONTENT_INDEX_FOOTER.len() as u64;

pub fn write_content_postings(path: impl AsRef<Path>, postings: &[ContentPosting]) -> Result<()> {
    let path = path.as_ref();
    let file = File::create(path).map_err(|err| GfmError::io(path, err))?;
    let mut writer = BufWriter::new(file);
    writer
        .write_all(CONTENT_MAGIC_V3)
        .map_err(|err| GfmError::io(path, err))?;
    write_varint(&mut writer, postings.len() as u64).map_err(|err| GfmError::io(path, err))?;

    let mut directory = Vec::with_capacity(postings.len());
    for posting in postings {
        let offset = writer
            .stream_position()
            .map_err(|err| GfmError::io(path, err))?;
        write_content_posting(&mut writer, posting).map_err(|err| GfmError::io(path, err))?;
        let end = writer
            .stream_position()
            .map_err(|err| GfmError::io(path, err))?;
        directory.push(ContentDirectoryEntry {
            term: posting.term.trim().to_lowercase(),
            offset,
            len: end.saturating_sub(offset),
        });
    }
    directory.sort_by(|left, right| left.term.cmp(&right.term));

    let directory_offset = writer
        .stream_position()
        .map_err(|err| GfmError::io(path, err))?;
    write_varint(&mut writer, directory.len() as u64).map_err(|err| GfmError::io(path, err))?;
    for entry in &directory {
        write_directory_entry(&mut writer, entry).map_err(|err| GfmError::io(path, err))?;
    }
    writer
        .write_all(&directory_offset.to_le_bytes())
        .map_err(|err| GfmError::io(path, err))?;
    writer
        .write_all(CONTENT_INDEX_FOOTER)
        .map_err(|err| GfmError::io(path, err))?;
    writer.flush().map_err(|err| GfmError::io(path, err))
}

pub fn read_content_postings(path: impl AsRef<Path>) -> Result<Vec<ContentPosting>> {
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    let magic = read_content_magic(&mut file, path)?;
    if magic != CONTENT_MAGIC_V1 && magic != CONTENT_MAGIC_V2 && magic != CONTENT_MAGIC_V3 {
        return Err(GfmError::Format(format!(
            "unsupported content store header in {}",
            path.display()
        )));
    }

    let count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
    let mut postings = Vec::with_capacity(count.min(1_000_000) as usize);
    for _ in 0..count {
        postings.push(read_content_posting(
            &mut file,
            path,
            magic.as_slice() == CONTENT_MAGIC_V3,
        )?);
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
    uses_positions: bool,
}

impl ContentArchive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        let magic = read_content_magic(&mut file, path)?;
        if magic == CONTENT_MAGIC_V2 || magic == CONTENT_MAGIC_V3 {
            let directory = read_content_directory(&mut file, path)?;
            Ok(Self {
                path: path.to_path_buf(),
                file,
                directory: ContentArchiveDirectory::Indexed(directory),
                uses_positions: magic == CONTENT_MAGIC_V3,
            })
        } else if magic == CONTENT_MAGIC_V1 {
            file.seek(SeekFrom::Start(CONTENT_MAGIC_V1.len() as u64))
                .map_err(|err| GfmError::io(path, err))?;
            let count = read_varint(&mut file).map_err(|err| GfmError::io(path, err))?;
            let mut postings = Vec::with_capacity(count.min(1_000_000) as usize);
            for _ in 0..count {
                postings.push(read_content_posting(&mut file, path, false)?);
            }
            Ok(Self {
                path: path.to_path_buf(),
                file,
                directory: ContentArchiveDirectory::Legacy(postings),
                uses_positions: false,
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
                    self.uses_positions,
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

pub fn write_content_segment(path: impl AsRef<Path>, segment: &ContentSegment) -> Result<()> {
    let path = path.as_ref();
    let file = File::create(path).map_err(|err| GfmError::io(path, err))?;
    let mut writer = BufWriter::new(file);
    writer
        .write_all(CONTENT_SEGMENT_MAGIC_V2)
        .map_err(|err| GfmError::io(path, err))?;
    write_file_ids(&mut writer, &segment.tombstones).map_err(|err| GfmError::io(path, err))?;
    write_varint(&mut writer, segment.postings.len() as u64)
        .map_err(|err| GfmError::io(path, err))?;
    for posting in &segment.postings {
        write_content_posting(&mut writer, posting).map_err(|err| GfmError::io(path, err))?;
    }
    writer.flush().map_err(|err| GfmError::io(path, err))
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
        postings.push(read_content_posting(&mut file, path, uses_positions)?);
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
    if len < CONTENT_MAGIC_V2.len() as u64 + CONTENT_FOOTER_LEN {
        return Err(content_format_error(
            path,
            "missing content directory footer",
        ));
    }

    file.seek(SeekFrom::End(-(CONTENT_FOOTER_LEN as i64)))
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

fn write_content_posting(mut writer: impl Write, posting: &ContentPosting) -> std::io::Result<()> {
    let term = posting.term.as_bytes();
    write_varint(&mut writer, term.len() as u64)?;
    writer.write_all(term)?;
    write_file_ids(&mut writer, &posting.ids)?;
    write_content_positions(writer, &posting.positions)
}

fn read_content_posting(
    mut reader: impl Read,
    path: &Path,
    uses_positions: bool,
) -> Result<ContentPosting> {
    let term_len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut term = vec![0; term_len as usize];
    reader
        .read_exact(&mut term)
        .map_err(|err| GfmError::io(path, err))?;
    let term = String::from_utf8(term).map_err(|err| {
        GfmError::Format(format!("invalid UTF-8 term in {}: {err}", path.display()))
    })?;
    let ids = read_file_ids(&mut reader, path)?;
    let positions = if uses_positions {
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
