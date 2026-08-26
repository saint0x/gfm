use super::{AccessIntent, SecurityDecisionAction, SecurityScopedAccessReport};
use gfm_types::{GfmError, Result};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const STORE_MAGIC: &str = "gfm-security-bookmarks-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityScopedBookmark {
    pub path: PathBuf,
    pub read_only: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityScopedBookmarkReport {
    pub path: PathBuf,
    pub status: SecurityScopedBookmarkStatus,
    pub read_only: bool,
    pub byte_len: usize,
    pub resolved_path: Option<PathBuf>,
    pub stale: bool,
    pub access_started: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityScopedBookmarkRecord {
    pub path: PathBuf,
    pub read_only: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityScopedBookmarkStore {
    path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityScopedBookmarkStoreReport {
    pub path: PathBuf,
    pub records: usize,
    pub repaired: usize,
    pub unavailable: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityScopedBookmarkLookup {
    pub requested_path: PathBuf,
    pub resolution: Option<SecurityScopedBookmarkResolution>,
}

pub struct SecurityScopedBookmarkAccess {
    pub report: SecurityScopedBookmarkReport,
    _native: gfm_mac_sys::NativeSecurityScopedAccess,
}

pub struct SecurityScopedBookmarkAccessLookup {
    pub requested_path: PathBuf,
    pub access: Option<SecurityScopedBookmarkAccess>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityScopedBookmarkResolution {
    pub record: SecurityScopedBookmarkRecord,
    pub report: SecurityScopedBookmarkReport,
    pub repaired: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityScopedBookmarkStatus {
    Created,
    Resolved,
    Missing,
    Unavailable,
    NotRequired,
}

impl SecurityScopedBookmarkStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Resolved => "resolved",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
            Self::NotRequired => "not-required",
        }
    }
}

impl SecurityScopedAccessReport {
    pub fn create_bookmark(&self) -> SecurityScopedBookmarkReport {
        if !self.bookmark_required {
            return SecurityScopedBookmarkReport::not_required(self.path.clone(), false);
        }
        if self.action != SecurityDecisionAction::Allow {
            return SecurityScopedBookmarkReport::unavailable(
                self.path.clone(),
                bookmark_read_only(self.intent),
                format!(
                    "bookmark creation requires allowed access; current action is {}",
                    self.action.as_str()
                ),
            );
        }
        SecurityScopedBookmark::create(&self.path, bookmark_read_only(self.intent))
            .map(SecurityScopedBookmarkReport::created)
            .unwrap_or_else(|report| report)
    }
}

impl SecurityScopedBookmark {
    pub fn create(
        path: impl AsRef<Path>,
        read_only: bool,
    ) -> std::result::Result<Self, SecurityScopedBookmarkReport> {
        let path = path.as_ref().to_path_buf();
        let native = gfm_mac_sys::create_security_scoped_bookmark(&path, read_only);
        match native.status {
            gfm_mac_sys::NativeBookmarkStatus::Available => Ok(Self {
                path,
                read_only,
                data: native.data,
            }),
            gfm_mac_sys::NativeBookmarkStatus::Missing => {
                Err(SecurityScopedBookmarkReport::missing(
                    path,
                    read_only,
                    native
                        .reason
                        .unwrap_or_else(|| "bookmark target missing".to_string()),
                ))
            }
            gfm_mac_sys::NativeBookmarkStatus::Unavailable => {
                Err(SecurityScopedBookmarkReport::unavailable(
                    path,
                    read_only,
                    native
                        .reason
                        .unwrap_or_else(|| "security-scoped bookmark unavailable".to_string()),
                ))
            }
        }
    }

    pub fn resolve(&self, start_access: bool) -> SecurityScopedBookmarkReport {
        let native = gfm_mac_sys::resolve_security_scoped_bookmark(&self.data, start_access);
        match native.status {
            gfm_mac_sys::NativeBookmarkStatus::Available => SecurityScopedBookmarkReport {
                path: self.path.clone(),
                status: SecurityScopedBookmarkStatus::Resolved,
                read_only: self.read_only,
                byte_len: self.data.len(),
                resolved_path: native.path,
                stale: native.stale,
                access_started: native.access_started,
                reason: None,
            },
            gfm_mac_sys::NativeBookmarkStatus::Missing => SecurityScopedBookmarkReport::missing(
                self.path.clone(),
                self.read_only,
                native
                    .reason
                    .unwrap_or_else(|| "bookmark target missing".to_string()),
            ),
            gfm_mac_sys::NativeBookmarkStatus::Unavailable => {
                SecurityScopedBookmarkReport::unavailable(
                    self.path.clone(),
                    self.read_only,
                    native.reason.unwrap_or_else(|| {
                        "security-scoped bookmark resolution unavailable".to_string()
                    }),
                )
            }
        }
    }

    pub fn start_access(
        &self,
    ) -> std::result::Result<SecurityScopedBookmarkAccess, SecurityScopedBookmarkReport> {
        match gfm_mac_sys::start_security_scoped_bookmark_access(&self.data) {
            Ok(native) => Ok(SecurityScopedBookmarkAccess {
                report: SecurityScopedBookmarkReport {
                    path: self.path.clone(),
                    status: SecurityScopedBookmarkStatus::Resolved,
                    read_only: self.read_only,
                    byte_len: self.data.len(),
                    resolved_path: native.path.clone(),
                    stale: native.stale,
                    access_started: true,
                    reason: None,
                },
                _native: native,
            }),
            Err(native) => Err(SecurityScopedBookmarkReport {
                path: self.path.clone(),
                status: match native.status {
                    gfm_mac_sys::NativeBookmarkStatus::Available => {
                        SecurityScopedBookmarkStatus::Resolved
                    }
                    gfm_mac_sys::NativeBookmarkStatus::Missing => {
                        SecurityScopedBookmarkStatus::Missing
                    }
                    gfm_mac_sys::NativeBookmarkStatus::Unavailable => {
                        SecurityScopedBookmarkStatus::Unavailable
                    }
                },
                read_only: self.read_only,
                byte_len: self.data.len(),
                resolved_path: native.path,
                stale: native.stale,
                access_started: native.access_started,
                reason: native.reason,
            }),
        }
    }
}

impl SecurityScopedBookmarkRecord {
    pub fn new(bookmark: SecurityScopedBookmark) -> Self {
        Self {
            path: bookmark.path,
            read_only: bookmark.read_only,
            data: bookmark.data,
        }
    }

    pub fn bookmark(&self) -> SecurityScopedBookmark {
        SecurityScopedBookmark {
            path: self.path.clone(),
            read_only: self.read_only,
            data: self.data.clone(),
        }
    }

    fn as_tsv(&self) -> String {
        format!(
            "bookmark\t{}\t{}\t{}",
            escape_field(&self.path.display().to_string()),
            self.read_only,
            hex_encode(&self.data)
        )
    }
}

impl SecurityScopedBookmarkStore {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_all(&self, records: &[SecurityScopedBookmarkRecord]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
        }
        let mut records = records.to_vec();
        records.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.read_only.cmp(&right.read_only))
        });
        records
            .dedup_by(|left, right| left.path == right.path && left.read_only == right.read_only);
        atomic_write(&self.path, |writer| {
            writeln!(writer, "{STORE_MAGIC}")?;
            for record in &records {
                writeln!(writer, "{}", record.as_tsv())?;
            }
            Ok(())
        })
    }

    pub fn read(&self) -> Result<Vec<SecurityScopedBookmarkRecord>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = fs::File::open(&self.path).map_err(|err| GfmError::io(&self.path, err))?;
        let mut reader = BufReader::new(file);
        let mut magic = String::new();
        reader
            .read_line(&mut magic)
            .map_err(|err| GfmError::io(&self.path, err))?;
        if magic.trim_end() != STORE_MAGIC {
            return Err(GfmError::Format(format!(
                "unsupported security bookmark store: {}",
                self.path.display()
            )));
        }
        let mut records = Vec::new();
        for (index, line) in reader.lines().enumerate() {
            let line = line.map_err(|err| GfmError::io(&self.path, err))?;
            if line.trim().is_empty() {
                continue;
            }
            records.push(parse_bookmark_record(&line).map_err(|err| {
                GfmError::Format(format!("{}:{}: {err}", self.path.display(), index + 2))
            })?);
        }
        Ok(records)
    }

    pub fn upsert(
        &self,
        bookmark: SecurityScopedBookmark,
    ) -> Result<SecurityScopedBookmarkStoreReport> {
        let mut records = self.read()?;
        let record = SecurityScopedBookmarkRecord::new(bookmark);
        if let Some(existing) = records
            .iter_mut()
            .find(|existing| existing.path == record.path && existing.read_only == record.read_only)
        {
            *existing = record;
        } else {
            records.push(record);
        }
        self.write_all(&records)?;
        Ok(SecurityScopedBookmarkStoreReport {
            path: self.path.clone(),
            records: records.len(),
            repaired: 0,
            unavailable: 0,
        })
    }

    pub fn resolve_all(
        &self,
        start_access: bool,
        repair_stale: bool,
    ) -> Result<Vec<SecurityScopedBookmarkResolution>> {
        let mut records = self.read()?;
        let mut resolutions = Vec::with_capacity(records.len());
        let mut repaired_any = false;
        for record in &mut records {
            let report = record.bookmark().resolve(start_access);
            let mut repaired = false;
            if repair_stale
                && report.status == SecurityScopedBookmarkStatus::Resolved
                && report.stale
                && report.resolved_path.is_some()
            {
                let resolved_path = report.resolved_path.clone().expect("checked above");
                if let Ok(bookmark) =
                    SecurityScopedBookmark::create(&resolved_path, record.read_only)
                {
                    record.path = bookmark.path;
                    record.data = bookmark.data;
                    repaired = true;
                    repaired_any = true;
                }
            }
            resolutions.push(SecurityScopedBookmarkResolution {
                record: record.clone(),
                report,
                repaired,
            });
        }
        if repaired_any {
            self.write_all(&records)?;
        }
        Ok(resolutions)
    }

    pub fn resolve_for_path(
        &self,
        path: impl AsRef<Path>,
        read_only: bool,
        start_access: bool,
        repair_stale: bool,
    ) -> Result<SecurityScopedBookmarkLookup> {
        let requested_path = path.as_ref().to_path_buf();
        let requested_identity = path_identity(&requested_path);
        let resolution = self
            .resolve_all(start_access, repair_stale)?
            .into_iter()
            .find(|resolution| {
                resolution.record.read_only == read_only
                    && same_path_identity(
                        &requested_identity,
                        &resolution.record.path,
                        resolution.report.resolved_path.as_deref(),
                    )
            });
        Ok(SecurityScopedBookmarkLookup {
            requested_path,
            resolution,
        })
    }

    pub fn start_access_for_path(
        &self,
        path: impl AsRef<Path>,
        read_only: bool,
        repair_stale: bool,
    ) -> Result<SecurityScopedBookmarkAccessLookup> {
        let requested_path = path.as_ref().to_path_buf();
        let lookup = self.resolve_for_path(&requested_path, read_only, false, repair_stale)?;
        let Some(resolution) = lookup.resolution else {
            return Ok(SecurityScopedBookmarkAccessLookup {
                requested_path,
                access: None,
            });
        };
        let access = resolution
            .record
            .bookmark()
            .start_access()
            .map_err(|report| GfmError::Permission {
                path: requested_path.clone(),
                message: report
                    .reason
                    .unwrap_or_else(|| "security-scoped access did not start".to_string()),
            })?;
        Ok(SecurityScopedBookmarkAccessLookup {
            requested_path,
            access: Some(access),
        })
    }

    pub fn reconcile(&self) -> Result<SecurityScopedBookmarkStoreReport> {
        let resolutions = self.resolve_all(false, true)?;
        Ok(SecurityScopedBookmarkStoreReport {
            path: self.path.clone(),
            records: resolutions.len(),
            repaired: resolutions
                .iter()
                .filter(|resolution| resolution.repaired)
                .count(),
            unavailable: resolutions
                .iter()
                .filter(|resolution| {
                    matches!(
                        resolution.report.status,
                        SecurityScopedBookmarkStatus::Missing
                            | SecurityScopedBookmarkStatus::Unavailable
                    )
                })
                .count(),
        })
    }
}

impl SecurityScopedBookmarkStoreReport {
    pub fn as_tsv(&self) -> String {
        format!(
            "security-bookmark-store\t{}\trecords={}\trepaired={}\tunavailable={}",
            self.path.display(),
            self.records,
            self.repaired,
            self.unavailable
        )
    }
}

impl SecurityScopedBookmarkReport {
    fn created(bookmark: SecurityScopedBookmark) -> Self {
        Self {
            path: bookmark.path,
            status: SecurityScopedBookmarkStatus::Created,
            read_only: bookmark.read_only,
            byte_len: bookmark.data.len(),
            resolved_path: None,
            stale: false,
            access_started: false,
            reason: None,
        }
    }

    fn not_required(path: PathBuf, read_only: bool) -> Self {
        Self {
            path,
            status: SecurityScopedBookmarkStatus::NotRequired,
            read_only,
            byte_len: 0,
            resolved_path: None,
            stale: false,
            access_started: false,
            reason: Some("path does not require a retained security-scoped bookmark".to_string()),
        }
    }

    fn missing(path: PathBuf, read_only: bool, reason: String) -> Self {
        Self {
            path,
            status: SecurityScopedBookmarkStatus::Missing,
            read_only,
            byte_len: 0,
            resolved_path: None,
            stale: false,
            access_started: false,
            reason: Some(reason),
        }
    }

    fn unavailable(path: PathBuf, read_only: bool, reason: String) -> Self {
        Self {
            path,
            status: SecurityScopedBookmarkStatus::Unavailable,
            read_only,
            byte_len: 0,
            resolved_path: None,
            stale: false,
            access_started: false,
            reason: Some(reason),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "security-bookmark\t{}\tstatus={}\tread-only={}\tbytes={}\tresolved={}\tstale={}\taccess-started={}\treason={}",
            self.path.display(),
            self.status.as_str(),
            self.read_only,
            self.byte_len,
            self.resolved_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.stale,
            self.access_started,
            self.reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string())
        )
    }
}

pub(super) fn bookmark_read_only(intent: AccessIntent) -> bool {
    matches!(
        intent,
        AccessIntent::Read | AccessIntent::Index | AccessIntent::Preview
    )
}

fn parse_bookmark_record(line: &str) -> std::result::Result<SecurityScopedBookmarkRecord, String> {
    let fields = line.split('\t').collect::<Vec<_>>();
    if fields.len() != 4 || fields[0] != "bookmark" {
        return Err("bookmark record must have 4 tab-separated fields".to_string());
    }
    let path = PathBuf::from(unescape_field(fields[1])?);
    let read_only = match fields[2] {
        "true" => true,
        "false" => false,
        other => {
            return Err(format!(
                "bookmark read-only flag must be true or false; got {other}"
            ))
        }
    };
    let data = hex_decode(fields[3])?;
    if data.is_empty() {
        return Err("bookmark data must not be empty".to_string());
    }
    Ok(SecurityScopedBookmarkRecord {
        path,
        read_only,
        data,
    })
}

fn same_path_identity(
    requested_identity: &PathBuf,
    record_path: &Path,
    resolved_path: Option<&Path>,
) -> bool {
    path_identity(record_path) == *requested_identity
        || resolved_path
            .map(path_identity)
            .is_some_and(|resolved_identity| resolved_identity == *requested_identity)
}

fn path_identity(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn unescape_field(value: &str) -> std::result::Result<String, String> {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(char) = chars.next() {
        if char != '\\' {
            output.push(char);
            continue;
        }
        match chars.next() {
            Some('\\') => output.push('\\'),
            Some('t') => output.push('\t'),
            Some('n') => output.push('\n'),
            Some(other) => return Err(format!("unknown escape sequence \\{other}")),
            None => return Err("unterminated escape sequence".to_string()),
        }
    }
    Ok(output)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn hex_decode(value: &str) -> std::result::Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("hex bookmark data must have even length".to_string());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(byte: u8) -> std::result::Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        other => Err(format!("invalid hex digit `{}`", other as char)),
    }
}

fn atomic_write(
    path: &Path,
    write: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
) -> Result<()> {
    let temporary = temporary_path(path);
    let file = fs::File::create(&temporary).map_err(|err| GfmError::io(&temporary, err))?;
    let mut writer = BufWriter::new(file);
    write(&mut writer).map_err(|err| GfmError::io(&temporary, err))?;
    writer
        .flush()
        .map_err(|err| GfmError::io(&temporary, err))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|err| GfmError::io(&temporary, err))?;
    drop(writer);
    fs::rename(&temporary, path).map_err(|err| GfmError::io(path, err))?;
    sync_parent(path);
    Ok(())
}

fn sync_parent(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    if let Ok(file) = fs::File::open(parent) {
        let _ = file.sync_all();
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("security-bookmarks");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()))
        .with_extension(format!("{nonce}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityScopedAccessReport;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn plain_paths_do_not_create_unnecessary_bookmarks() {
        let root = temp_root("security-bookmark-plain");
        let path = root.join("note.md");
        fs::write(&path, "note").unwrap();
        let report = SecurityScopedAccessReport::evaluate(&path, AccessIntent::Read);

        let bookmark = report.create_bookmark();

        assert_eq!(bookmark.status, SecurityScopedBookmarkStatus::NotRequired);
        assert_eq!(bookmark.byte_len, 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protected_allowed_paths_create_and_resolve_bookmarks() {
        let root = temp_root("security-bookmark-documents");
        let path = root.join("Documents").join("Plan.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "plan").unwrap();
        let report = SecurityScopedAccessReport::evaluate(&path, AccessIntent::Read);

        let bookmark = SecurityScopedBookmark::create(&path, true).unwrap();
        let created = report.create_bookmark();
        let resolved = bookmark.resolve(false);

        assert_eq!(created.status, SecurityScopedBookmarkStatus::Created);
        assert!(created.byte_len > 0);
        assert_eq!(resolved.status, SecurityScopedBookmarkStatus::Resolved);
        assert_eq!(
            resolved
                .resolved_path
                .as_ref()
                .and_then(|path| path.canonicalize().ok()),
            Some(path.canonicalize().unwrap())
        );
        assert!(!resolved.stale);
        assert!(resolved.as_tsv().contains("status=resolved"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bookmark_store_round_trips_records_atomically_and_sorted() {
        let root = temp_root("security-bookmark-store");
        let store = SecurityScopedBookmarkStore::new(root.join("nested").join("bookmarks.tsv"));
        let first_path = root.join("Documents").join("Beta.md");
        let second_path = root.join("Desktop").join("Alpha.md");
        fs::create_dir_all(first_path.parent().unwrap()).unwrap();
        fs::create_dir_all(second_path.parent().unwrap()).unwrap();
        fs::write(&first_path, "beta").unwrap();
        fs::write(&second_path, "alpha").unwrap();

        let first = SecurityScopedBookmark::create(&first_path, true).unwrap();
        let second = SecurityScopedBookmark::create(&second_path, true).unwrap();

        let first_report = store.upsert(first).unwrap();
        let second_report = store.upsert(second).unwrap();
        let records = store.read().unwrap();

        assert_eq!(first_report.records, 1);
        assert_eq!(second_report.records, 2);
        assert_eq!(records.len(), 2);
        assert!(records[0].path < records[1].path);
        assert!(fs::read_to_string(store.path())
            .unwrap()
            .starts_with(STORE_MAGIC));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bookmark_store_resolves_and_reconciles_records() {
        let root = temp_root("security-bookmark-reconcile");
        let store = SecurityScopedBookmarkStore::new(root.join("bookmarks.tsv"));
        let path = root.join("Documents").join("Plan.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "plan").unwrap();
        let bookmark = SecurityScopedBookmark::create(&path, true).unwrap();
        store.upsert(bookmark).unwrap();

        let resolutions = store.resolve_all(false, true).unwrap();
        let report = store.reconcile().unwrap();

        assert_eq!(resolutions.len(), 1);
        assert_eq!(
            resolutions[0]
                .report
                .resolved_path
                .as_ref()
                .and_then(|path| path.canonicalize().ok()),
            Some(path.canonicalize().unwrap())
        );
        assert_eq!(report.records, 1);
        assert_eq!(report.unavailable, 0);
        assert!(report.as_tsv().contains("records=1"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bookmark_store_resolves_single_record_by_canonical_identity() {
        let root = temp_root("security-bookmark-lookup");
        let store = SecurityScopedBookmarkStore::new(root.join("bookmarks.tsv"));
        let path = root.join("Documents").join("Plan.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "plan").unwrap();
        store
            .upsert(SecurityScopedBookmark::create(&path, true).unwrap())
            .unwrap();

        let lookup = store.resolve_for_path(&path, true, false, true).unwrap();

        assert!(lookup.resolution.is_some());
        assert_eq!(
            lookup
                .resolution
                .as_ref()
                .unwrap()
                .report
                .resolved_path
                .as_ref()
                .and_then(|path| path.canonicalize().ok()),
            Some(path.canonicalize().unwrap())
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bookmark_store_starts_scoped_access_for_matching_record() {
        let root = temp_root("security-bookmark-access");
        let store = SecurityScopedBookmarkStore::new(root.join("bookmarks.tsv"));
        let path = root.join("Documents").join("Plan.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "plan").unwrap();
        store
            .upsert(SecurityScopedBookmark::create(&path, false).unwrap())
            .unwrap();

        let lookup = store.start_access_for_path(&path, false, true).unwrap();
        let access = lookup.access.expect("matching bookmark access");

        assert_eq!(lookup.requested_path, path);
        assert_eq!(access.report.status, SecurityScopedBookmarkStatus::Resolved);
        assert!(access.report.access_started);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bookmark_store_rejects_corrupt_records() {
        let root = temp_root("security-bookmark-corrupt");
        let path = root.join("bookmarks.tsv");
        fs::write(&path, format!("{STORE_MAGIC}\nbookmark\t/a\ttrue\tnope\n")).unwrap();
        let store = SecurityScopedBookmarkStore::new(&path);

        let error = store.read().unwrap_err();

        assert!(format!("{error:?}").contains("invalid hex digit"));

        fs::remove_dir_all(root).unwrap();
    }

    fn temp_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gfm-{name}-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
