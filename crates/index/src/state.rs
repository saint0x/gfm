use gfm_types::{DirectoryPage, GfmError, Result, VolumeId};
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

pub const INDEX_STATE_SCHEMA_VERSION: u32 = 1;

const MAGIC: &str = "gfm-index-state-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexVolumeState {
    pub schema_version: u32,
    pub root: PathBuf,
    pub records_path: PathBuf,
    pub volume_id: VolumeId,
    pub mount_id: String,
    pub scan_epoch: u64,
    pub record_count: usize,
    pub inaccessible_count: usize,
}

impl IndexVolumeState {
    pub fn from_page(
        page: &DirectoryPage,
        records_path: impl Into<PathBuf>,
        previous: Option<&Self>,
    ) -> Result<Self> {
        let root_record = page.entries.iter().find(|record| record.path == page.root);
        let volume_id = root_record
            .or_else(|| page.entries.first())
            .map(|record| record.id.volume)
            .ok_or_else(|| {
                GfmError::Format(format!(
                    "cannot persist index state for empty scan rooted at {}",
                    page.root.display()
                ))
            })?;
        let mount_id = mount_identity(&page.root, volume_id);
        let scan_epoch = previous
            .filter(|state| {
                state.schema_version == INDEX_STATE_SCHEMA_VERSION
                    && state.volume_id == volume_id
                    && state.mount_id == mount_id
            })
            .map(|state| state.scan_epoch.saturating_add(1))
            .unwrap_or(1);

        Ok(Self {
            schema_version: INDEX_STATE_SCHEMA_VERSION,
            root: page.root.clone(),
            records_path: records_path.into(),
            volume_id,
            mount_id,
            scan_epoch,
            record_count: page.entries.len(),
            inaccessible_count: page.inaccessible.len(),
        })
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        gfm_store::atomic_write(path, |writer| {
            let mut writer = BufWriter::new(writer);
            writeln!(writer, "{MAGIC}")?;
            writeln!(writer, "schema_version\t{}", self.schema_version)?;
            writeln!(writer, "root\t{}", escape_path(&self.root))?;
            writeln!(writer, "records_path\t{}", escape_path(&self.records_path))?;
            writeln!(writer, "volume_id\t{}", self.volume_id.0)?;
            writeln!(writer, "mount_id\t{}", escape(&self.mount_id))?;
            writeln!(writer, "scan_epoch\t{}", self.scan_epoch)?;
            writeln!(writer, "record_count\t{}", self.record_count)?;
            writeln!(writer, "inaccessible_count\t{}", self.inaccessible_count)?;
            writer.flush()
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
                    "unsupported index state header `{header}` in {}",
                    path.display()
                )))
            }
            Some(Err(err)) => return Err(GfmError::io(path, err)),
            None => {
                return Err(GfmError::Format(format!(
                    "empty index state {}",
                    path.display()
                )))
            }
        }
        check_control()?;

        let mut schema_version = None;
        let mut root = None;
        let mut records_path = None;
        let mut volume_id = None;
        let mut mount_id = None;
        let mut scan_epoch = None;
        let mut record_count = None;
        let mut inaccessible_count = None;
        let mut seen_fields = BTreeSet::new();

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
            if !seen_fields.insert(key.to_string()) {
                return Err(GfmError::Format(format!(
                    "{} line {}: duplicate index state field `{key}`",
                    path.display(),
                    line_index + 2
                )));
            }
            match key {
                "schema_version" => {
                    schema_version = Some(parse_u32(value, "schema_version", path)?)
                }
                "root" => root = Some(PathBuf::from(unescape(value)?)),
                "records_path" => records_path = Some(PathBuf::from(unescape(value)?)),
                "volume_id" => volume_id = Some(VolumeId(parse_u64(value, "volume_id", path)?)),
                "mount_id" => mount_id = Some(unescape(value)?),
                "scan_epoch" => scan_epoch = Some(parse_u64(value, "scan_epoch", path)?),
                "record_count" => record_count = Some(parse_usize(value, "record_count", path)?),
                "inaccessible_count" => {
                    inaccessible_count = Some(parse_usize(value, "inaccessible_count", path)?)
                }
                other => {
                    return Err(GfmError::Format(format!(
                        "{}: unknown index state field `{other}`",
                        path.display()
                    )))
                }
            }
        }
        check_control()?;

        let state = Self {
            schema_version: required(schema_version, "schema_version", path)?,
            root: required(root, "root", path)?,
            records_path: required(records_path, "records_path", path)?,
            volume_id: required(volume_id, "volume_id", path)?,
            mount_id: required(mount_id, "mount_id", path)?,
            scan_epoch: required(scan_epoch, "scan_epoch", path)?,
            record_count: required(record_count, "record_count", path)?,
            inaccessible_count: required(inaccessible_count, "inaccessible_count", path)?,
        };
        if state.schema_version != INDEX_STATE_SCHEMA_VERSION {
            return Err(GfmError::Format(format!(
                "{}: unsupported index state schema version {}",
                path.display(),
                state.schema_version
            )));
        }
        check_control()?;
        Ok(state)
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "index-state\troot={}\trecords-path={}\tschema={}\tvolume={}\tmount={}\tscan-epoch={}\trecord-count={}\tinaccessible-count={}",
            self.root.display(),
            self.records_path.display(),
            self.schema_version,
            self.volume_id.0,
            self.mount_id,
            self.scan_epoch,
            self.record_count,
            self.inaccessible_count
        )
    }
}

fn mount_identity(root: &Path, volume_id: VolumeId) -> String {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    format!("dev:{}:root:{}", volume_id.0, canonical.display())
}

fn required<T>(value: Option<T>, field: &str, path: &Path) -> Result<T> {
    value.ok_or_else(|| {
        GfmError::Format(format!(
            "{}: missing index state field `{field}`",
            path.display()
        ))
    })
}

fn parse_u32(value: &str, field: &str, path: &Path) -> Result<u32> {
    value.parse().map_err(|err| {
        GfmError::Format(format!(
            "{}: invalid index state {field} `{value}`: {err}",
            path.display()
        ))
    })
}

fn parse_u64(value: &str, field: &str, path: &Path) -> Result<u64> {
    value.parse().map_err(|err| {
        GfmError::Format(format!(
            "{}: invalid index state {field} `{value}`: {err}",
            path.display()
        ))
    })
}

fn parse_usize(value: &str, field: &str, path: &Path) -> Result<usize> {
    value.parse().map_err(|err| {
        GfmError::Format(format!(
            "{}: invalid index state {field} `{value}`: {err}",
            path.display()
        ))
    })
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
                    "invalid index state escape `\\{other}`"
                )))
            }
            None => return Err(GfmError::Format("trailing index state escape".to_string())),
        }
    }
    Ok(output)
}
