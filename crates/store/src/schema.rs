use crate::{
    ContentArchiveManifest, MmapContentArchive, MmapDictionary, MmapFuzzyArchive,
    MmapMetadataArchive, MmapPrefixArchive, MmapRecordArchive, MmapRecordColumns,
};
use gfm_types::Result;
use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

const RECORD_CURRENT: &str = "gfm-store-v3";
const RECORD_LEGACY: &[&str] = &["gfm-store-v1", "gfm-store-v2"];
const COLUMNS_CURRENT: &[u8] = b"gfm-record-columns-v2\n";
const COLUMNS_LEGACY: &[&[u8]] = &[b"gfm-record-columns-v1\n"];
const METADATA_CURRENT: &[u8] = b"gfm-metadata-v3\n";
const METADATA_LEGACY: &[&[u8]] = &[b"gfm-metadata-v1\n", b"gfm-metadata-v2\n"];
const PREFIX_CURRENT: &[u8] = b"gfm-prefix-v1\n";
const FUZZY_CURRENT: &[u8] = b"gfm-fuzzy-v1\n";
const DICTIONARY_CURRENT: &[u8] = b"gfm-dictionary-v1\n";
const CONTENT_CURRENT: &[u8] = b"gfm-content-v5\n";
const CONTENT_LEGACY: &[&[u8]] = &[
    b"gfm-content-v1\n",
    b"gfm-content-v2\n",
    b"gfm-content-v3\n",
    b"gfm-content-v4\n",
];
const CONTENT_MANIFEST_CURRENT: &str = "gfm-content-manifest-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveSchemaKind {
    Records,
    Columns,
    Metadata,
    Prefixes,
    Fuzzy,
    Dictionary,
    Content,
    ContentManifest,
}

impl ArchiveSchemaKind {
    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "records" => Some(Self::Records),
            "columns" => Some(Self::Columns),
            "metadata" => Some(Self::Metadata),
            "prefixes" | "prefix" => Some(Self::Prefixes),
            "fuzzy" => Some(Self::Fuzzy),
            "dictionary" => Some(Self::Dictionary),
            "content" => Some(Self::Content),
            "content-manifest" => Some(Self::ContentManifest),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Records => "records",
            Self::Columns => "columns",
            Self::Metadata => "metadata",
            Self::Prefixes => "prefixes",
            Self::Fuzzy => "fuzzy",
            Self::Dictionary => "dictionary",
            Self::Content => "content",
            Self::ContentManifest => "content-manifest",
        }
    }

    pub const fn current_schema(self) -> &'static str {
        match self {
            Self::Records => RECORD_CURRENT,
            Self::Columns => "gfm-record-columns-v2",
            Self::Metadata => "gfm-metadata-v3",
            Self::Prefixes => "gfm-prefix-v1",
            Self::Fuzzy => "gfm-fuzzy-v1",
            Self::Dictionary => "gfm-dictionary-v1",
            Self::Content => "gfm-content-v5",
            Self::ContentManifest => CONTENT_MANIFEST_CURRENT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveSchemaStatus {
    Current,
    Legacy,
    Unsupported,
    Missing,
    Unreadable,
}

impl ArchiveSchemaStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Legacy => "legacy",
            Self::Unsupported => "unsupported",
            Self::Missing => "missing",
            Self::Unreadable => "unreadable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveSchemaReport {
    pub kind: ArchiveSchemaKind,
    pub path: PathBuf,
    pub status: ArchiveSchemaStatus,
    pub schema: Option<String>,
    pub current_schema: &'static str,
    pub detail: Option<String>,
}

impl ArchiveSchemaReport {
    pub fn as_tsv(&self) -> String {
        format!(
            "archive-schema\tkind={}\tstatus={}\tschema={}\tcurrent={}\tpath={}\tdetail={}",
            self.kind.as_str(),
            self.status.as_str(),
            self.schema.as_deref().unwrap_or("-"),
            self.current_schema,
            escape_field(&self.path.display().to_string()),
            self.detail
                .as_deref()
                .map(escape_field)
                .unwrap_or("-".to_string())
        )
    }
}

pub fn inspect_archive_schema(
    kind: ArchiveSchemaKind,
    path: impl AsRef<Path>,
) -> ArchiveSchemaReport {
    let path = path.as_ref();
    match inspect_archive_schema_result(kind, path) {
        Ok(report) => report,
        Err(detail) => ArchiveSchemaReport {
            kind,
            path: path.to_path_buf(),
            status: ArchiveSchemaStatus::Unreadable,
            schema: None,
            current_schema: kind.current_schema(),
            detail: Some(detail.to_string()),
        },
    }
}

fn inspect_archive_schema_result(
    kind: ArchiveSchemaKind,
    path: &Path,
) -> Result<ArchiveSchemaReport> {
    if !path.exists() {
        return Ok(report(kind, path, ArchiveSchemaStatus::Missing, None, None));
    }
    match kind {
        ArchiveSchemaKind::Records => {
            inspect_line_schema(kind, path, &[RECORD_CURRENT], RECORD_LEGACY, || {
                MmapRecordArchive::open(path).map(|_| ())
            })
        }
        ArchiveSchemaKind::Columns => {
            inspect_magic_schema(kind, path, &[COLUMNS_CURRENT], COLUMNS_LEGACY, || {
                MmapRecordColumns::open(path).map(|_| ())
            })
        }
        ArchiveSchemaKind::Metadata => {
            inspect_magic_schema(kind, path, &[METADATA_CURRENT], METADATA_LEGACY, || {
                MmapMetadataArchive::open(path).map(|_| ())
            })
        }
        ArchiveSchemaKind::Prefixes => {
            inspect_magic_schema(kind, path, &[PREFIX_CURRENT], &[], || {
                MmapPrefixArchive::open(path).map(|_| ())
            })
        }
        ArchiveSchemaKind::Fuzzy => inspect_magic_schema(kind, path, &[FUZZY_CURRENT], &[], || {
            MmapFuzzyArchive::open(path).map(|_| ())
        }),
        ArchiveSchemaKind::Dictionary => {
            inspect_magic_schema(kind, path, &[DICTIONARY_CURRENT], &[], || {
                MmapDictionary::open(path).map(|_| ())
            })
        }
        ArchiveSchemaKind::Content => {
            inspect_magic_schema(kind, path, &[CONTENT_CURRENT], CONTENT_LEGACY, || {
                MmapContentArchive::open(path).map(|_| ())
            })
        }
        ArchiveSchemaKind::ContentManifest => {
            inspect_line_schema(kind, path, &[CONTENT_MANIFEST_CURRENT], &[], || {
                ContentArchiveManifest::read(path).map(|_| ())
            })
        }
    }
}

fn inspect_line_schema(
    kind: ArchiveSchemaKind,
    path: &Path,
    current: &[&str],
    legacy: &[&str],
    validate: impl FnOnce() -> Result<()>,
) -> Result<ArchiveSchemaReport> {
    let file = File::open(path).map_err(|err| gfm_types::GfmError::io(path, err))?;
    let mut lines = BufReader::new(file).lines();
    let Some(line) = lines.next() else {
        return Ok(report(
            kind,
            path,
            ArchiveSchemaStatus::Unsupported,
            None,
            Some("empty archive".to_string()),
        ));
    };
    let schema = line.map_err(|err| gfm_types::GfmError::io(path, err))?;
    let status = if current.contains(&schema.as_str()) {
        ArchiveSchemaStatus::Current
    } else if legacy.contains(&schema.as_str()) {
        ArchiveSchemaStatus::Legacy
    } else {
        return Ok(report(
            kind,
            path,
            ArchiveSchemaStatus::Unsupported,
            Some(schema),
            None,
        ));
    };
    validated_report(kind, path, status, Some(schema), validate)
}

fn inspect_magic_schema(
    kind: ArchiveSchemaKind,
    path: &Path,
    current: &[&[u8]],
    legacy: &[&[u8]],
    validate: impl FnOnce() -> Result<()>,
) -> Result<ArchiveSchemaReport> {
    let max_len = current
        .iter()
        .chain(legacy.iter())
        .map(|magic| magic.len())
        .max()
        .unwrap_or(0);
    let mut bytes = vec![0; max_len];
    let mut file = File::open(path).map_err(|err| gfm_types::GfmError::io(path, err))?;
    let len = file
        .read(&mut bytes)
        .map_err(|err| gfm_types::GfmError::io(path, err))?;
    bytes.truncate(len);

    if let Some(schema) = matching_magic(&bytes, current) {
        return validated_report(
            kind,
            path,
            ArchiveSchemaStatus::Current,
            Some(schema),
            validate,
        );
    }
    if let Some(schema) = matching_magic(&bytes, legacy) {
        return validated_report(
            kind,
            path,
            ArchiveSchemaStatus::Legacy,
            Some(schema),
            validate,
        );
    }
    Ok(report(
        kind,
        path,
        ArchiveSchemaStatus::Unsupported,
        None,
        Some("unsupported archive header".to_string()),
    ))
}

fn validated_report(
    kind: ArchiveSchemaKind,
    path: &Path,
    status: ArchiveSchemaStatus,
    schema: Option<String>,
    validate: impl FnOnce() -> Result<()>,
) -> Result<ArchiveSchemaReport> {
    match validate() {
        Ok(()) => Ok(report(kind, path, status, schema, None)),
        Err(err) => Ok(report(
            kind,
            path,
            ArchiveSchemaStatus::Unreadable,
            schema,
            Some(err.to_string()),
        )),
    }
}

fn matching_magic(bytes: &[u8], candidates: &[&[u8]]) -> Option<String> {
    candidates
        .iter()
        .find(|magic| bytes.starts_with(magic))
        .map(|magic| String::from_utf8_lossy(magic).trim_end().to_string())
}

fn report(
    kind: ArchiveSchemaKind,
    path: &Path,
    status: ArchiveSchemaStatus,
    schema: Option<String>,
    detail: Option<String>,
) -> ArchiveSchemaReport {
    ArchiveSchemaReport {
        kind,
        path: path.to_path_buf(),
        status,
        schema,
        current_schema: kind.current_schema(),
        detail,
    }
}

fn escape_field(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

impl fmt::Display for ArchiveSchemaStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        metadata_postings_from_records, prefix_postings_from_records, write_content_postings,
        write_dictionary, write_fuzzy_postings, write_metadata_postings, write_prefix_postings,
        write_record_columns, write_records, ContentArchiveManifestEntry, ContentMergeTier,
    };
    use gfm_types::{ContentPosting, FileId, FileKind, FileRecord, VolumeId};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn inspects_current_archive_schemas_with_real_readers() {
        let records = temp_path("gfm-schema-records", "gfmidx");
        let columns = temp_path("gfm-schema-columns", "gfmcols");
        let metadata = temp_path("gfm-schema-metadata", "gfmmeta");
        let prefixes = temp_path("gfm-schema-prefixes", "gfmprefix");
        let fuzzy = temp_path("gfm-schema-fuzzy", "gfmfuzzy");
        let dictionary = temp_path("gfm-schema-dictionary", "gfmdict");
        let content = temp_path("gfm-schema-content", "gfmcontent");
        let manifest = temp_path("gfm-schema-manifest", "gfmmanifest");
        let rows = vec![record()];

        write_records(&records, &rows).unwrap();
        write_record_columns(&columns, &rows).unwrap();
        write_metadata_postings(&metadata, &metadata_postings_from_records(&rows)).unwrap();
        write_prefix_postings(&prefixes, &prefix_postings_from_records(&rows)).unwrap();
        write_fuzzy_postings(&fuzzy, &crate::fuzzy_postings_from_records(&rows)).unwrap();
        write_dictionary(&dictionary, &crate::dictionary_terms_from_records(&rows)).unwrap();
        write_content_postings(
            &content,
            &[ContentPosting {
                term: "instant".to_string(),
                ids: vec![FileId::new(VolumeId(1), 2)],
                positions: Vec::new(),
            }],
        )
        .unwrap();
        ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path: content.clone(),
        }])
        .unwrap()
        .write(&manifest)
        .unwrap();

        for (kind, path) in [
            (ArchiveSchemaKind::Records, records.as_path()),
            (ArchiveSchemaKind::Columns, columns.as_path()),
            (ArchiveSchemaKind::Metadata, metadata.as_path()),
            (ArchiveSchemaKind::Prefixes, prefixes.as_path()),
            (ArchiveSchemaKind::Fuzzy, fuzzy.as_path()),
            (ArchiveSchemaKind::Dictionary, dictionary.as_path()),
            (ArchiveSchemaKind::Content, content.as_path()),
            (ArchiveSchemaKind::ContentManifest, manifest.as_path()),
        ] {
            let report = inspect_archive_schema(kind, path);
            assert_eq!(report.status, ArchiveSchemaStatus::Current, "{report:?}");
            assert_eq!(report.schema.as_deref(), Some(kind.current_schema()));
        }

        for path in [
            records, columns, metadata, prefixes, fuzzy, dictionary, content, manifest,
        ] {
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn classifies_legacy_missing_unsupported_and_unreadable_archives() {
        let legacy = temp_path("gfm-schema-legacy", "gfmidx");
        let missing = temp_path("gfm-schema-missing", "gfmidx");
        let unsupported = temp_path("gfm-schema-unsupported", "gfmidx");
        let corrupt = temp_path("gfm-schema-corrupt", "gfmprefix");

        std::fs::write(
            &legacy,
            "gfm-store-v1\n1\t2\t0\tf\t1\t0\t0\t0\t0\t/tmp/legacy.txt\n",
        )
        .unwrap();
        std::fs::write(&unsupported, "not-gfm\nbody\n").unwrap();
        std::fs::write(&corrupt, PREFIX_CURRENT).unwrap();

        assert_eq!(
            inspect_archive_schema(ArchiveSchemaKind::Records, &legacy).status,
            ArchiveSchemaStatus::Legacy
        );
        assert_eq!(
            inspect_archive_schema(ArchiveSchemaKind::Records, &missing).status,
            ArchiveSchemaStatus::Missing
        );
        assert_eq!(
            inspect_archive_schema(ArchiveSchemaKind::Records, &unsupported).status,
            ArchiveSchemaStatus::Unsupported
        );
        let corrupt_report = inspect_archive_schema(ArchiveSchemaKind::Prefixes, &corrupt);
        assert_eq!(corrupt_report.status, ArchiveSchemaStatus::Unreadable);
        assert_eq!(corrupt_report.schema.as_deref(), Some("gfm-prefix-v1"));

        std::fs::remove_file(legacy).unwrap();
        std::fs::remove_file(unsupported).unwrap();
        std::fs::remove_file(corrupt).unwrap();
    }

    fn record() -> FileRecord {
        FileRecord {
            id: FileId::new(VolumeId(1), 2),
            parent: Some(FileId::new(VolumeId(1), 1)),
            path: PathBuf::from("/tmp/schema/Instant Search.md"),
            name: "Instant Search.md".to_string(),
            kind: FileKind::File,
            len: 128,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 7,
            created: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
            modified: Some(UNIX_EPOCH + Duration::from_secs(2)),
            changed: None,
            hidden: false,
            tags: vec!["fast".to_string()],
            finder_comment: Some("instant lookup".to_string()),
        }
    }

    fn temp_path(prefix: &str, extension: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("{prefix}-{nanos}.{extension}"));
        path
    }
}
