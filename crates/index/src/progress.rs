use gfm_types::{GfmError, Result};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCAN_PROGRESS_SCHEMA_VERSION: u32 = 1;

const MAGIC: &str = "gfm-scan-progress-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanProgressCheckpoint {
    pub schema_version: u32,
    pub root: PathBuf,
    pub records_path: PathBuf,
    pub started_at_nanos: u128,
    pub updated_at_nanos: u128,
    pub scanned_records: usize,
    pub inaccessible_records: usize,
    pub published_segments: usize,
    pub tombstones: usize,
    pub last_path: Option<PathBuf>,
    pub completed: bool,
}

impl ScanProgressCheckpoint {
    pub fn started(root: impl Into<PathBuf>, records_path: impl Into<PathBuf>) -> Self {
        let now = now_nanos();
        Self {
            schema_version: SCAN_PROGRESS_SCHEMA_VERSION,
            root: root.into(),
            records_path: records_path.into(),
            started_at_nanos: now,
            updated_at_nanos: now,
            scanned_records: 0,
            inaccessible_records: 0,
            published_segments: 0,
            tombstones: 0,
            last_path: None,
            completed: false,
        }
    }

    pub fn with_progress(
        mut self,
        scanned_records: usize,
        inaccessible_records: usize,
        last_path: Option<PathBuf>,
    ) -> Self {
        self.updated_at_nanos = now_nanos();
        self.scanned_records = scanned_records;
        self.inaccessible_records = inaccessible_records;
        self.last_path = last_path;
        self
    }

    pub fn with_publication(mut self, published_segments: usize, tombstones: usize) -> Self {
        self.updated_at_nanos = now_nanos();
        self.published_segments = published_segments;
        self.tombstones = tombstones;
        self
    }

    pub fn completed(mut self) -> Self {
        self.updated_at_nanos = now_nanos();
        self.completed = true;
        self
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        self.write_checked(path, || Ok(()))
    }

    pub fn write_checked(
        &self,
        path: impl AsRef<Path>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<()> {
        let path = path.as_ref();
        gfm_store::atomic_write_checked(path, &mut check_control, |writer, check_control| {
            let mut writer = BufWriter::new(writer);
            macro_rules! line {
                ($($arg:tt)*) => {
                    writeln!($($arg)*).map_err(|err| GfmError::io(path, err))?
                };
            }
            check_control()?;
            line!(writer, "{MAGIC}");
            check_control()?;
            line!(writer, "schema_version\t{}", self.schema_version);
            line!(writer, "root\t{}", escape_path(&self.root));
            line!(writer, "records_path\t{}", escape_path(&self.records_path));
            line!(writer, "started_at_nanos\t{}", self.started_at_nanos);
            line!(writer, "updated_at_nanos\t{}", self.updated_at_nanos);
            line!(writer, "scanned_records\t{}", self.scanned_records);
            line!(
                writer,
                "inaccessible_records\t{}",
                self.inaccessible_records
            );
            line!(writer, "published_segments\t{}", self.published_segments);
            line!(writer, "tombstones\t{}", self.tombstones);
            line!(
                writer,
                "last_path\t{}",
                self.last_path
                    .as_deref()
                    .map(escape_path)
                    .unwrap_or_default()
            );
            line!(writer, "completed\t{}", self.completed);
            check_control()?;
            writer.flush().map_err(|err| GfmError::io(path, err))
        })
        .map(|_| ())
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        Self::read_checked(path, || Ok(()))
    }

    pub fn read_checked(
        path: impl AsRef<Path>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let path = path.as_ref();
        check_control()?;
        let file = fs::File::open(path).map_err(|err| GfmError::io(path, err))?;
        check_control()?;
        let mut lines = BufReader::new(file).lines();
        match lines.next() {
            Some(Ok(header)) if header == MAGIC => {}
            Some(Ok(header)) => {
                return Err(GfmError::Format(format!(
                    "unsupported scan progress header `{header}` in {}",
                    path.display()
                )))
            }
            Some(Err(err)) => return Err(GfmError::io(path, err)),
            None => {
                return Err(GfmError::Format(format!(
                    "empty scan progress {}",
                    path.display()
                )))
            }
        }
        check_control()?;

        let mut schema_version = None;
        let mut root = None;
        let mut records_path = None;
        let mut started_at_nanos = None;
        let mut updated_at_nanos = None;
        let mut scanned_records = None;
        let mut inaccessible_records = None;
        let mut published_segments = None;
        let mut tombstones = None;
        let mut last_path = None;
        let mut completed = None;

        for (line_index, line) in lines.enumerate() {
            check_control()?;
            let line = line.map_err(|err| GfmError::io(path, err))?;
            check_control()?;
            let (key, value) = line.split_once('\t').ok_or_else(|| {
                GfmError::Format(format!(
                    "{} line {}: expected key and value",
                    path.display(),
                    line_index + 2
                ))
            })?;
            match key {
                "schema_version" => {
                    schema_version = Some(parse_u32(value, "schema_version", path)?)
                }
                "root" => root = Some(PathBuf::from(unescape(value)?)),
                "records_path" => records_path = Some(PathBuf::from(unescape(value)?)),
                "started_at_nanos" => {
                    started_at_nanos = Some(parse_u128(value, "started_at_nanos", path)?)
                }
                "updated_at_nanos" => {
                    updated_at_nanos = Some(parse_u128(value, "updated_at_nanos", path)?)
                }
                "scanned_records" => {
                    scanned_records = Some(parse_usize(value, "scanned_records", path)?)
                }
                "inaccessible_records" => {
                    inaccessible_records = Some(parse_usize(value, "inaccessible_records", path)?)
                }
                "published_segments" => {
                    published_segments = Some(parse_usize(value, "published_segments", path)?)
                }
                "tombstones" => tombstones = Some(parse_usize(value, "tombstones", path)?),
                "last_path" => {
                    last_path = if value.is_empty() {
                        Some(None)
                    } else {
                        Some(Some(PathBuf::from(unescape(value)?)))
                    }
                }
                "completed" => completed = Some(parse_bool(value, "completed", path)?),
                other => {
                    return Err(GfmError::Format(format!(
                        "{}: unknown scan progress field `{other}`",
                        path.display()
                    )))
                }
            }
        }
        check_control()?;

        let checkpoint = Self {
            schema_version: required(schema_version, "schema_version", path)?,
            root: required(root, "root", path)?,
            records_path: required(records_path, "records_path", path)?,
            started_at_nanos: required(started_at_nanos, "started_at_nanos", path)?,
            updated_at_nanos: required(updated_at_nanos, "updated_at_nanos", path)?,
            scanned_records: required(scanned_records, "scanned_records", path)?,
            inaccessible_records: required(inaccessible_records, "inaccessible_records", path)?,
            published_segments: required(published_segments, "published_segments", path)?,
            tombstones: required(tombstones, "tombstones", path)?,
            last_path: required(last_path, "last_path", path)?,
            completed: required(completed, "completed", path)?,
        };
        if checkpoint.schema_version != SCAN_PROGRESS_SCHEMA_VERSION {
            return Err(GfmError::Format(format!(
                "{}: unsupported scan progress schema version {}",
                path.display(),
                checkpoint.schema_version
            )));
        }
        check_control()?;
        Ok(checkpoint)
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "scan-progress\troot={}\trecords-path={}\tscanned={}\tinaccessible={}\tsegments={}\ttombstones={}\tlast-path={}\tcompleted={}",
            self.root.display(),
            self.records_path.display(),
            self.scanned_records,
            self.inaccessible_records,
            self.published_segments,
            self.tombstones,
            self.last_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.completed
        )
    }
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn required<T>(value: Option<T>, field: &str, path: &Path) -> Result<T> {
    value.ok_or_else(|| {
        GfmError::Format(format!(
            "{}: missing scan progress field `{field}`",
            path.display()
        ))
    })
}

fn parse_u32(value: &str, field: &str, path: &Path) -> Result<u32> {
    value
        .parse()
        .map_err(|err| parse_error(field, value, path, err))
}

fn parse_u128(value: &str, field: &str, path: &Path) -> Result<u128> {
    value
        .parse()
        .map_err(|err| parse_error(field, value, path, err))
}

fn parse_usize(value: &str, field: &str, path: &Path) -> Result<usize> {
    value
        .parse()
        .map_err(|err| parse_error(field, value, path, err))
}

fn parse_bool(value: &str, field: &str, path: &Path) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(GfmError::Format(format!(
            "{}: invalid scan progress {field} `{value}`",
            path.display()
        ))),
    }
}

fn parse_error(field: &str, value: &str, path: &Path, err: impl std::fmt::Display) -> GfmError {
    GfmError::Format(format!(
        "{}: invalid scan progress {field} `{value}`: {err}",
        path.display()
    ))
}

fn escape_path(path: &Path) -> String {
    escape(&path.to_string_lossy())
}

fn escape(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            other => output.push(other),
        }
    }
    output
}

fn unescape(input: &str) -> Result<String> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => output.push('\\'),
            Some('t') => output.push('\t'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some(other) => {
                return Err(GfmError::Format(format!(
                    "invalid scan progress escape `\\{other}`"
                )))
            }
            None => {
                return Err(GfmError::Format(
                    "trailing scan progress escape".to_string(),
                ))
            }
        }
    }
    Ok(output)
}
