mod content;
mod durable;

pub use content::{
    compact_content_segments, read_content_postings, read_content_segment, write_content_postings,
    write_content_segment, ContentArchive,
};
pub use durable::{atomic_write, DurableCommit};

use gfm_types::{FileId, FileKind, FileRecord, GfmError, Result, VolumeId};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAGIC_V1: &str = "gfm-store-v1";
const MAGIC_V2: &str = "gfm-store-v2";
const MAGIC_V3: &str = "gfm-store-v3";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoreVersion {
    V1,
    V2,
    V3,
}

pub fn write_records(path: impl AsRef<Path>, records: &[FileRecord]) -> Result<()> {
    let path = path.as_ref();
    durable::atomic_write(path, |writer| {
        writeln!(writer, "{MAGIC_V3}")?;
        for record in records {
            writeln!(
                writer,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                record.id.volume.0,
                record.id.node,
                record.parent.map(|id| id.node).unwrap_or(0),
                encode_kind(record.kind),
                record.len,
                record.mode,
                record.owner,
                record.group,
                record.xattrs_digest,
                encode_time(record.created),
                encode_time(record.modified),
                encode_time(record.changed),
                u8::from(record.hidden),
                encode_tags(&record.tags),
                record
                    .finder_comment
                    .as_deref()
                    .map(escape)
                    .unwrap_or_default(),
                escape(&record.path.to_string_lossy()),
            )?;
        }
        Ok(())
    })
    .map(|_| ())
}

pub fn read_records(path: impl AsRef<Path>) -> Result<Vec<FileRecord>> {
    let path = path.as_ref();
    let file = std::fs::File::open(path).map_err(|err| GfmError::io(path, err))?;
    let mut lines = BufReader::new(file).lines();
    let version = match lines.next() {
        Some(Ok(header)) if header == MAGIC_V1 => StoreVersion::V1,
        Some(Ok(header)) if header == MAGIC_V2 => StoreVersion::V2,
        Some(Ok(header)) if header == MAGIC_V3 => StoreVersion::V3,
        Some(Ok(header)) => {
            return Err(GfmError::Format(format!(
                "unsupported store header `{header}` in {}",
                path.display()
            )));
        }
        Some(Err(err)) => return Err(GfmError::io(path, err)),
        None => return Err(GfmError::Format(format!("empty store {}", path.display()))),
    };

    let mut records = Vec::new();
    for (index, line) in lines.enumerate() {
        let line = line.map_err(|err| GfmError::io(path, err))?;
        records.push(parse_record(&line, version).map_err(|err| {
            GfmError::Format(format!("{} line {}: {}", path.display(), index + 2, err))
        })?);
    }
    Ok(records)
}

fn parse_record(line: &str, version: StoreVersion) -> std::result::Result<FileRecord, String> {
    let parts: Vec<_> = line.split('\t').collect();
    let expected = match version {
        StoreVersion::V1 => 10,
        StoreVersion::V2 => 11,
        StoreVersion::V3 => 16,
    };
    if parts.len() != expected {
        return Err(format!("expected {expected} fields, got {}", parts.len()));
    }

    let volume = parse_u64(parts[0], "volume")?;
    let node = parse_u64(parts[1], "node")?;
    let parent_node = parse_u64(parts[2], "parent")?;
    let kind = decode_kind(parts[3])?;
    let len = parse_u64(parts[4], "len")?;
    let (mode, owner, group, xattrs_digest, created_index) = match version {
        StoreVersion::V1 | StoreVersion::V2 => (0, 0, 0, 0, 5),
        StoreVersion::V3 => (
            parse_u32(parts[5], "mode")?,
            parse_u32(parts[6], "owner")?,
            parse_u32(parts[7], "group")?,
            parse_u64(parts[8], "xattrs_digest")?,
            9,
        ),
    };
    let created = decode_time(parts[created_index])?;
    let modified = decode_time(parts[created_index + 1])?;
    let changed = decode_time(parts[created_index + 2])?;
    let hidden = match parts[created_index + 3] {
        "0" => false,
        "1" => true,
        other => return Err(format!("invalid hidden flag `{other}`")),
    };
    let (tags, finder_comment, path_index) = match version {
        StoreVersion::V1 => (Vec::new(), None, 9),
        StoreVersion::V2 => (decode_tags(parts[9])?, None, 10),
        StoreVersion::V3 => (
            decode_tags(parts[created_index + 4])?,
            decode_comment(parts[created_index + 5])?,
            created_index + 6,
        ),
    };
    let path = PathBuf::from(unescape(parts[path_index])?);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string());

    Ok(FileRecord {
        id: FileId::new(VolumeId(volume), node),
        parent: (parent_node != 0).then_some(FileId::new(VolumeId(volume), parent_node)),
        path,
        name,
        kind,
        len,
        mode,
        owner,
        group,
        xattrs_digest,
        created,
        modified,
        changed,
        hidden,
        tags,
        finder_comment,
    })
}

fn parse_u64(value: &str, field: &str) -> std::result::Result<u64, String> {
    value
        .parse()
        .map_err(|err| format!("invalid {field} `{value}`: {err}"))
}

fn parse_u32(value: &str, field: &str) -> std::result::Result<u32, String> {
    value
        .parse()
        .map_err(|err| format!("invalid {field} `{value}`: {err}"))
}

fn encode_kind(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "d",
        FileKind::File => "f",
        FileKind::Symlink => "l",
        FileKind::Other => "o",
    }
}

fn decode_kind(value: &str) -> std::result::Result<FileKind, String> {
    match value {
        "d" => Ok(FileKind::Directory),
        "f" => Ok(FileKind::File),
        "l" => Ok(FileKind::Symlink),
        "o" => Ok(FileKind::Other),
        other => Err(format!("invalid file kind `{other}`")),
    }
}

fn encode_time(time: Option<SystemTime>) -> u128 {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn encode_tags(tags: &[String]) -> String {
    tags.iter()
        .map(|tag| escape(tag))
        .collect::<Vec<_>>()
        .join(",")
}

fn decode_tags(input: &str) -> std::result::Result<Vec<String>, String> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let mut tags: Vec<_> = split_escaped(input)
        .into_iter()
        .map(unescape)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|tag| !tag.trim().is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    Ok(tags)
}

fn decode_comment(input: &str) -> std::result::Result<Option<String>, String> {
    if input.is_empty() {
        return Ok(None);
    }
    Ok(Some(unescape(input)?))
}

fn split_escaped(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else if ch == ',' {
            parts.push(&input[start..index]);
            start = index + ch.len_utf8();
        }
    }
    parts.push(&input[start..]);
    parts
}

fn decode_time(value: &str) -> std::result::Result<Option<SystemTime>, String> {
    let nanos: u128 = value
        .parse()
        .map_err(|err| format!("invalid timestamp `{value}`: {err}"))?;
    Ok((nanos != 0)
        .then_some(UNIX_EPOCH + Duration::from_nanos(nanos.min(u64::MAX as u128) as u64)))
}

fn escape(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            ',' => output.push_str("\\,"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            other => output.push(other),
        }
    }
    output
}

fn unescape(input: &str) -> std::result::Result<String, String> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => output.push('\\'),
            Some(',') => output.push(','),
            Some('t') => output.push('\t'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some(other) => return Err(format!("invalid escape `\\{other}`")),
            None => return Err("trailing escape".to_string()),
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_records() {
        let path = temp_path("gfm-store", "idx");
        let records = vec![FileRecord {
            id: FileId::new(VolumeId(4), 12),
            parent: Some(FileId::new(VolumeId(4), 1)),
            path: PathBuf::from("/tmp/a b/report.txt"),
            name: "report.txt".to_string(),
            kind: FileKind::File,
            len: 42,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 99,
            created: None,
            modified: Some(UNIX_EPOCH + Duration::from_secs(10)),
            changed: None,
            hidden: false,
            tags: vec!["Important".to_string(), "Review, Later".to_string()],
            finder_comment: Some("handoff notes".to_string()),
        }];

        write_records(&path, &records).unwrap();
        let read = read_records(&path).unwrap();

        assert_eq!(read, records);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reads_legacy_record_store_without_tags() {
        let path = temp_path("gfm-store-legacy", "idx");
        std::fs::write(
            &path,
            "gfm-store-v1\n4\t12\t1\tf\t42\t0\t0\t0\t0\t/tmp/legacy.txt\n",
        )
        .unwrap();

        let read = read_records(&path).unwrap();

        assert_eq!(read.len(), 1);
        assert_eq!(read[0].name, "legacy.txt");
        assert!(read[0].tags.is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn atomic_write_preserves_existing_file_on_write_failure() {
        let path = temp_path("gfm-store-atomic", "txt");
        std::fs::write(&path, "stable").unwrap();

        let result = atomic_write(&path, |writer| {
            writer.write_all(b"partial")?;
            Err(std::io::Error::other("simulated crash"))
        });

        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "stable");
        std::fs::remove_file(path).unwrap();
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
