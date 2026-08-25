use crate::IndexVolumeState;
use gfm_types::{GfmError, Result, VolumeId};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

pub const FSEVENTS_CURSOR_SCHEMA_VERSION: u32 = 1;

const MAGIC: &str = "gfm-fsevents-cursor-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FseventsCursorHealth {
    Clean,
    RepairRequired,
}

impl FseventsCursorHealth {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::RepairRequired => "repair-required",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "clean" | "ok" => Ok(Self::Clean),
            "repair-required" | "repair" => Ok(Self::RepairRequired),
            other => Err(GfmError::Format(format!(
                "unsupported FSEvents cursor health `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FseventsCursor {
    pub schema_version: u32,
    pub volume_id: VolumeId,
    pub mount_id: String,
    pub scan_epoch: u64,
    pub last_event_id: u64,
    pub health: FseventsCursorHealth,
}

impl FseventsCursor {
    pub fn checkpoint(
        volume: &IndexVolumeState,
        last_event_id: u64,
        health: FseventsCursorHealth,
    ) -> Self {
        Self {
            schema_version: FSEVENTS_CURSOR_SCHEMA_VERSION,
            volume_id: volume.volume_id,
            mount_id: volume.mount_id.clone(),
            scan_epoch: volume.scan_epoch,
            last_event_id,
            health,
        }
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        gfm_store::atomic_write(path, |writer| {
            let mut writer = BufWriter::new(writer);
            writeln!(writer, "{MAGIC}")?;
            writeln!(writer, "schema_version\t{}", self.schema_version)?;
            writeln!(writer, "volume_id\t{}", self.volume_id.0)?;
            writeln!(writer, "mount_id\t{}", escape(&self.mount_id))?;
            writeln!(writer, "scan_epoch\t{}", self.scan_epoch)?;
            writeln!(writer, "last_event_id\t{}", self.last_event_id)?;
            writeln!(writer, "health\t{}", self.health.as_str())?;
            writer.flush()
        })
        .map(|_| ())
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = fs::File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mut lines = BufReader::new(file).lines();
        match lines.next() {
            Some(Ok(header)) if header == MAGIC => {}
            Some(Ok(header)) => {
                return Err(GfmError::Format(format!(
                    "unsupported FSEvents cursor header `{header}` in {}",
                    path.display()
                )))
            }
            Some(Err(err)) => return Err(GfmError::io(path, err)),
            None => {
                return Err(GfmError::Format(format!(
                    "empty FSEvents cursor {}",
                    path.display()
                )))
            }
        }

        let mut schema_version = None;
        let mut volume_id = None;
        let mut mount_id = None;
        let mut scan_epoch = None;
        let mut last_event_id = None;
        let mut health = None;
        for (line_index, line) in lines.enumerate() {
            let line = line.map_err(|err| GfmError::io(path, err))?;
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
                "volume_id" => volume_id = Some(VolumeId(parse_u64(value, "volume_id", path)?)),
                "mount_id" => mount_id = Some(unescape(value)?),
                "scan_epoch" => scan_epoch = Some(parse_u64(value, "scan_epoch", path)?),
                "last_event_id" => last_event_id = Some(parse_u64(value, "last_event_id", path)?),
                "health" => health = Some(FseventsCursorHealth::parse(value)?),
                other => {
                    return Err(GfmError::Format(format!(
                        "{}: unknown FSEvents cursor field `{other}`",
                        path.display()
                    )))
                }
            }
        }

        let cursor = Self {
            schema_version: required(schema_version, "schema_version", path)?,
            volume_id: required(volume_id, "volume_id", path)?,
            mount_id: required(mount_id, "mount_id", path)?,
            scan_epoch: required(scan_epoch, "scan_epoch", path)?,
            last_event_id: required(last_event_id, "last_event_id", path)?,
            health: required(health, "health", path)?,
        };
        if cursor.schema_version != FSEVENTS_CURSOR_SCHEMA_VERSION {
            return Err(GfmError::Format(format!(
                "{}: unsupported FSEvents cursor schema version {}",
                path.display(),
                cursor.schema_version
            )));
        }
        Ok(cursor)
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "fsevents-cursor\tschema={}\tvolume={}\tmount={}\tscan-epoch={}\tlast-event-id={}\thealth={}",
            self.schema_version,
            self.volume_id.0,
            self.mount_id,
            self.scan_epoch,
            self.last_event_id,
            self.health.as_str()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FseventsResumeAction {
    Continue,
    Rescan,
}

impl FseventsResumeAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Rescan => "rescan",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FseventsResumePlan {
    pub action: FseventsResumeAction,
    pub from_event_id: Option<u64>,
    pub reason: String,
}

impl FseventsResumePlan {
    pub fn evaluate(volume: &IndexVolumeState, cursor: Option<&FseventsCursor>) -> Self {
        let Some(cursor) = cursor else {
            return Self::rescan("missing-cursor");
        };
        if cursor.schema_version != FSEVENTS_CURSOR_SCHEMA_VERSION {
            return Self::rescan("schema-mismatch");
        }
        if cursor.volume_id != volume.volume_id {
            return Self::rescan("volume-changed");
        }
        if cursor.mount_id != volume.mount_id {
            return Self::rescan("mount-changed");
        }
        if cursor.scan_epoch != volume.scan_epoch {
            return Self::rescan("scan-epoch-changed");
        }
        if cursor.health == FseventsCursorHealth::RepairRequired {
            return Self::rescan("repair-required");
        }
        Self {
            action: FseventsResumeAction::Continue,
            from_event_id: Some(cursor.last_event_id.saturating_add(1)),
            reason: "cursor-clean".to_string(),
        }
    }

    pub fn read(volume: &IndexVolumeState, cursor_path: impl AsRef<Path>) -> Result<Self> {
        let cursor_path = cursor_path.as_ref();
        let cursor = cursor_path
            .exists()
            .then(|| FseventsCursor::read(cursor_path))
            .transpose()?;
        Ok(Self::evaluate(volume, cursor.as_ref()))
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "fsevents-resume\taction={}\tfrom-event-id={}\treason={}",
            self.action.as_str(),
            self.from_event_id
                .map(|event_id| event_id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.reason
        )
    }

    fn rescan(reason: &str) -> Self {
        Self {
            action: FseventsResumeAction::Rescan,
            from_event_id: None,
            reason: reason.to_string(),
        }
    }
}

fn required<T>(value: Option<T>, field: &str, path: &Path) -> Result<T> {
    value.ok_or_else(|| {
        GfmError::Format(format!(
            "{}: missing FSEvents cursor field `{field}`",
            path.display()
        ))
    })
}

fn parse_u32(value: &str, field: &str, path: &Path) -> Result<u32> {
    value.parse().map_err(|err| {
        GfmError::Format(format!(
            "{}: invalid FSEvents cursor {field} `{value}`: {err}",
            path.display()
        ))
    })
}

fn parse_u64(value: &str, field: &str, path: &Path) -> Result<u64> {
    value.parse().map_err(|err| {
        GfmError::Format(format!(
            "{}: invalid FSEvents cursor {field} `{value}`: {err}",
            path.display()
        ))
    })
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
                    "invalid FSEvents cursor escape `\\{other}`"
                )))
            }
            None => {
                return Err(GfmError::Format(
                    "trailing FSEvents cursor escape".to_string(),
                ))
            }
        }
    }
    Ok(output)
}
