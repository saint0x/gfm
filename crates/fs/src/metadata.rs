use crate::{record_for_path, PackageKind, PackagePolicy};
use gfm_types::{FileKind, FileRecord, Result, SecondaryMetadataRecord};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

const USER_TAGS_XATTR: &str = "com.apple.metadata:_kMDItemUserTags";
const FINDER_COMMENT_XATTR: &str = "com.apple.metadata:kMDItemFinderComment";
const FINDER_INFO_XATTR: &str = "com.apple.FinderInfo";
const FINDER_FLAG_EXTENSION_HIDDEN: u16 = 0x0010;
const FINDER_FLAG_ALIAS: u16 = 0x8000;
const LOCALIZED_SIDECAR_READ_LIMIT: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinderLabelColor {
    None,
    Gray,
    Green,
    Purple,
    Blue,
    Yellow,
    Red,
    Orange,
}

impl FinderLabelColor {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gray => "gray",
            Self::Green => "green",
            Self::Purple => "purple",
            Self::Blue => "blue",
            Self::Yellow => "yellow",
            Self::Red => "red",
            Self::Orange => "orange",
        }
    }

    fn from_index(index: u8) -> Self {
        match index {
            1 => Self::Gray,
            2 => Self::Green,
            3 => Self::Purple,
            4 => Self::Blue,
            5 => Self::Yellow,
            6 => Self::Red,
            7 => Self::Orange,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinderTagEntry {
    pub name: String,
    pub color: FinderLabelColor,
}

impl FinderTagEntry {
    pub fn new(name: impl Into<String>, color: FinderLabelColor) -> Self {
        Self {
            name: name.into(),
            color,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinderTypeRole {
    Folder,
    Package(PackageKind),
    Document,
    UnixExecutable,
    Symlink,
    Alias,
    Other,
}

impl FinderTypeRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Package(kind) => kind.as_str(),
            Self::Document => "document",
            Self::UnixExecutable => "unix-executable",
            Self::Symlink => "symlink",
            Self::Alias => "alias",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinderLinkRole {
    None,
    Symlink,
    Alias,
}

impl FinderLinkRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Symlink => "symlink",
            Self::Alias => "alias",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinderMetadataReport {
    pub record: FileRecord,
    pub display_name: String,
    pub localized_name: Option<String>,
    pub extension_hidden: bool,
    pub hidden: bool,
    pub tags: Vec<FinderTagEntry>,
    pub label: FinderLabelColor,
    pub comment: Option<String>,
    pub type_role: FinderTypeRole,
    pub kind_string: String,
    pub link_role: FinderLinkRole,
}

impl FinderMetadataReport {
    pub fn read_path(path: impl AsRef<Path>) -> Result<Self> {
        let record = record_for_path(path.as_ref(), None, false)?;
        Ok(Self::from_record(record))
    }

    pub fn from_record(record: FileRecord) -> Self {
        let finder_info = FinderInfo::read(&record.path);
        let tags = finder_tag_entries(&record.path);
        let label = tags
            .iter()
            .find(|tag| tag.color != FinderLabelColor::None)
            .map(|tag| tag.color)
            .unwrap_or_else(|| finder_info.label);
        let link_role = link_role(&record, &finder_info);
        let type_role = type_role(&record, link_role);
        let localized_name = localized_name(&record.path);
        let comment = finder_comment(&record.path);
        let kind_string = kind_string(&record, type_role);
        let hidden = record.hidden || extension_hidden_by_finder_policy(&record.name);
        let extension_hidden =
            finder_info.extension_hidden || extension_hidden_by_name_policy(&record.name);
        let display_name = bundle_display_name(&record, type_role)
            .or_else(|| localized_name.clone())
            .unwrap_or_else(|| display_name(&record.name, extension_hidden));

        Self {
            record,
            display_name,
            localized_name,
            extension_hidden,
            hidden,
            tags,
            label,
            comment,
            type_role,
            kind_string,
            link_role,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "finder-metadata\t{}\tdisplay={}\tlocalized={}\tkind={}\ttype={}\tlink={}\thidden={}\text-hidden={}\tlabel={}\ttags={}\tcomment={}",
            self.record.path.display(),
            escape_field(&self.display_name),
            self.localized_name.as_deref().map(escape_field).unwrap_or_else(|| "-".to_string()),
            escape_field(&self.kind_string),
            self.type_role.as_str(),
            self.link_role.as_str(),
            self.hidden,
            self.extension_hidden,
            self.label.as_str(),
            self.tags.len(),
            self.comment.as_deref().map(escape_field).unwrap_or_else(|| "-".to_string()),
        )];
        lines.extend(
            self.tags
                .iter()
                .map(|tag| format!("tag\t{}\t{}", escape_field(&tag.name), tag.color.as_str())),
        );
        lines.join("\n")
    }

    pub fn secondary_metadata_record(&self) -> SecondaryMetadataRecord {
        let mut comments = vec![
            self.display_name.clone(),
            self.kind_string.clone(),
            self.type_role.as_str().to_string(),
            self.link_role.as_str().to_string(),
            self.label.as_str().to_string(),
        ];
        comments.extend(self.localized_name.iter().cloned());
        comments.extend(self.comment.iter().cloned());
        comments.retain(|value| !value.trim().is_empty());
        comments.sort();
        comments.dedup();

        let mut tags = self
            .tags
            .iter()
            .map(|tag| tag.name.clone())
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();

        SecondaryMetadataRecord {
            id: self.record.id,
            tags,
            comments,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FinderInfo {
    label: FinderLabelColor,
    extension_hidden: bool,
    alias: bool,
}

impl FinderInfo {
    fn read(path: &Path) -> Self {
        let Some(raw) = xattr::get(path, FINDER_INFO_XATTR).ok().flatten() else {
            return Self::default();
        };
        Self::from_bytes(&raw)
    }

    fn from_bytes(raw: &[u8]) -> Self {
        if raw.len() < 10 {
            return Self::default();
        }
        let flags = u16::from_be_bytes([raw[8], raw[9]]);
        let label_index = (raw[9] >> 1) & 0x07;
        Self {
            label: FinderLabelColor::from_index(label_index),
            extension_hidden: flags & FINDER_FLAG_EXTENSION_HIDDEN != 0,
            alias: flags & FINDER_FLAG_ALIAS != 0,
        }
    }
}

impl Default for FinderInfo {
    fn default() -> Self {
        Self {
            label: FinderLabelColor::None,
            extension_hidden: false,
            alias: false,
        }
    }
}

fn finder_tag_entries(path: &Path) -> Vec<FinderTagEntry> {
    let Some(raw) = xattr::get(path, USER_TAGS_XATTR).ok().flatten() else {
        return Vec::new();
    };
    let Ok(plist::Value::Array(values)) = plist::Value::from_reader(std::io::Cursor::new(raw))
    else {
        return Vec::new();
    };

    let mut tags: Vec<_> = values
        .into_iter()
        .filter_map(|value| match value {
            plist::Value::String(tag) => finder_tag_entry(&tag),
            _ => None,
        })
        .collect();
    tags.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.color.as_str().cmp(right.color.as_str()))
    });
    tags.dedup_by(|left, right| left.name == right.name && left.color == right.color);
    tags
}

fn finder_tag_entry(raw: &str) -> Option<FinderTagEntry> {
    let (name, color) = raw
        .split_once('\n')
        .map(|(name, color)| (name, color.parse::<u8>().unwrap_or(0)))
        .unwrap_or((raw, 0));
    let name = name.trim();
    (!name.is_empty()).then(|| FinderTagEntry::new(name, FinderLabelColor::from_index(color)))
}

fn finder_comment(path: &Path) -> Option<String> {
    let raw = xattr::get(path, FINDER_COMMENT_XATTR).ok().flatten()?;
    match plist::Value::from_reader(std::io::Cursor::new(raw)).ok()? {
        plist::Value::String(value) => non_empty(value),
        _ => None,
    }
}

fn localized_name(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    let name = path.file_name()?.to_str()?;
    let localized = parent.join(".localized");
    let file = std::fs::File::open(localized).ok()?;
    BufReader::new(file)
        .take(LOCALIZED_SIDECAR_READ_LIMIT)
        .lines()
        .find_map(|line| {
            let line = line.ok()?;
            let (source, translated) = line.split_once('\t')?;
            (source == name)
                .then(|| translated.trim().to_string())
                .and_then(non_empty)
        })
}

fn bundle_display_name(record: &FileRecord, role: FinderTypeRole) -> Option<String> {
    if !matches!(role, FinderTypeRole::Package(_)) {
        return None;
    }
    let plist = record.path.join("Contents").join("Info.plist");
    let value = plist::Value::from_file(plist).ok()?;
    let plist::Value::Dictionary(dictionary) = value else {
        return None;
    };
    dictionary
        .get("CFBundleDisplayName")
        .and_then(plist_string)
        .or_else(|| dictionary.get("CFBundleName").and_then(plist_string))
        .and_then(non_empty)
}

fn plist_string(value: &plist::Value) -> Option<String> {
    match value {
        plist::Value::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn display_name(name: &str, extension_hidden: bool) -> String {
    if extension_hidden {
        Path::new(name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .unwrap_or(name)
            .to_string()
    } else {
        name.to_string()
    }
}

fn extension_hidden_by_name_policy(name: &str) -> bool {
    name.ends_with(".app") || name.ends_with(".localized")
}

fn extension_hidden_by_finder_policy(name: &str) -> bool {
    name == ".localized"
}

fn type_role(record: &FileRecord, link_role: FinderLinkRole) -> FinderTypeRole {
    if link_role == FinderLinkRole::Alias {
        return FinderTypeRole::Alias;
    }
    if record.kind == FileKind::Symlink {
        return FinderTypeRole::Symlink;
    }
    if let Some(package_kind) = PackagePolicy::default().classify(&record.path, record.kind) {
        return FinderTypeRole::Package(package_kind);
    }
    match record.kind {
        FileKind::Directory => FinderTypeRole::Folder,
        FileKind::File if is_probable_unix_executable(record) => FinderTypeRole::UnixExecutable,
        FileKind::File => FinderTypeRole::Document,
        FileKind::Symlink => FinderTypeRole::Symlink,
        FileKind::Other => FinderTypeRole::Other,
    }
}

fn link_role(record: &FileRecord, finder_info: &FinderInfo) -> FinderLinkRole {
    if finder_info.alias
        || record
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("alias"))
    {
        FinderLinkRole::Alias
    } else if record.kind == FileKind::Symlink {
        FinderLinkRole::Symlink
    } else {
        FinderLinkRole::None
    }
}

fn kind_string(record: &FileRecord, role: FinderTypeRole) -> String {
    match role {
        FinderTypeRole::Folder => "Folder".to_string(),
        FinderTypeRole::Package(PackageKind::Application) => "Application".to_string(),
        FinderTypeRole::Package(PackageKind::Framework) => "Framework".to_string(),
        FinderTypeRole::Package(PackageKind::XcodeProject) => "Xcode Project".to_string(),
        FinderTypeRole::Package(PackageKind::PhotosLibrary) => "Photos Library".to_string(),
        FinderTypeRole::Package(PackageKind::DocumentPackage) => extension_title(record)
            .map(|extension| format!("{extension} Document"))
            .unwrap_or_else(|| "Package".to_string()),
        FinderTypeRole::Package(_) => "Package".to_string(),
        FinderTypeRole::Document => extension_title(record)
            .map(|extension| format!("{extension} Document"))
            .unwrap_or_else(|| "Document".to_string()),
        FinderTypeRole::UnixExecutable => "Unix Executable File".to_string(),
        FinderTypeRole::Symlink => "Alias".to_string(),
        FinderTypeRole::Alias => "Alias".to_string(),
        FinderTypeRole::Other => "Item".to_string(),
    }
}

fn extension_title(record: &FileRecord) -> Option<String> {
    let extension = record.extension()?.trim();
    if extension.is_empty() {
        return None;
    }
    let mut chars = extension.chars();
    let first = chars.next()?.to_uppercase().collect::<String>();
    Some(format!("{}{}", first, chars.as_str().to_ascii_lowercase()))
}

#[cfg(unix)]
fn is_probable_unix_executable(record: &FileRecord) -> bool {
    use std::os::unix::fs::PermissionsExt;

    if record.kind != FileKind::File {
        return false;
    }
    std::fs::metadata(&record.path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_probable_unix_executable(_record: &FileRecord) -> bool {
    false
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn reports_tags_comments_labels_and_hidden_extension() {
        let root = unique_temp_dir();
        let path = root.join("Report.md");
        fs::write(&path, "report").unwrap();
        set_tags(&path, &["Important\n6", "Client\n4"]);
        set_comment(&path, "handoff notes");
        set_finder_info(&path, 6, true, false);

        let report = FinderMetadataReport::read_path(&path).unwrap();

        assert_eq!(report.display_name, "Report");
        assert_eq!(report.label, FinderLabelColor::Blue);
        assert_eq!(report.comment.as_deref(), Some("handoff notes"));
        assert_eq!(report.tags.len(), 2);
        assert!(report.extension_hidden);
        assert!(report.as_tsv().contains("tag\tImportant\tred"));
        assert!(report.as_tsv().contains("comment=handoff notes"));

        let secondary = report.secondary_metadata_record();
        assert_eq!(secondary.id, report.record.id);
        assert_eq!(
            secondary.tags,
            vec!["Client".to_string(), "Important".to_string()]
        );
        assert!(secondary.comments.contains(&"Report".to_string()));
        assert!(secondary.comments.contains(&"Md Document".to_string()));
        assert!(secondary.comments.contains(&"handoff notes".to_string()));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn classifies_packages_symlinks_aliases_and_localized_names() {
        let root = unique_temp_dir();
        let app = root.join("GFM.app");
        fs::create_dir_all(app.join("Contents")).unwrap();
        let mut info = plist::Dictionary::new();
        info.insert(
            "CFBundleDisplayName".to_string(),
            plist::Value::String("Good Finder Manager".to_string()),
        );
        plist::Value::Dictionary(info)
            .to_file_xml(app.join("Contents").join("Info.plist"))
            .unwrap();
        fs::write(root.join(".localized"), "GFM.app\tGood Fucking Manager\n").unwrap();

        let app_report = FinderMetadataReport::read_path(&app).unwrap();

        assert_eq!(app_report.display_name, "Good Finder Manager");
        assert_eq!(
            app_report.localized_name.as_deref(),
            Some("Good Fucking Manager")
        );
        assert_eq!(
            app_report.type_role,
            FinderTypeRole::Package(PackageKind::Application)
        );
        assert_eq!(app_report.kind_string, "Application");
        assert!(app_report
            .secondary_metadata_record()
            .comments
            .contains(&"Good Finder Manager".to_string()));

        let target = root.join("target.txt");
        let link = root.join("target link");
        fs::write(&target, "target").unwrap();
        make_symlink(&target, &link);
        let link_report = FinderMetadataReport::read_path(&link).unwrap();
        assert_eq!(link_report.link_role, FinderLinkRole::Symlink);
        assert_eq!(link_report.kind_string, "Alias");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn localized_name_reads_bounded_sidecar_prefix() {
        let root = unique_temp_dir();
        let document = root.join("Guide.txt");
        fs::write(&document, "localized metadata").unwrap();
        let mut localized = String::from("Guide.txt\tLocalized Guide\n");
        localized.push_str(&"Other.txt\tIgnored\n".repeat(8192));
        fs::write(root.join(".localized"), localized).unwrap();

        let report = FinderMetadataReport::read_path(&document).unwrap();

        assert_eq!(report.localized_name.as_deref(), Some("Localized Guide"));
        assert_eq!(report.display_name, "Localized Guide");

        fs::remove_dir_all(root).unwrap();
    }

    fn set_tags(path: &Path, tags: &[&str]) {
        let value = plist::Value::Array(
            tags.iter()
                .map(|tag| plist::Value::String((*tag).to_string()))
                .collect(),
        );
        let mut payload = Vec::new();
        value.to_writer_binary(&mut payload).unwrap();
        let _ = xattr::set(path, USER_TAGS_XATTR, &payload);
    }

    fn set_comment(path: &Path, comment: &str) {
        let mut payload = Vec::new();
        plist::Value::String(comment.to_string())
            .to_writer_binary(&mut payload)
            .unwrap();
        let _ = xattr::set(path, FINDER_COMMENT_XATTR, &payload);
    }

    fn set_finder_info(path: &Path, label: u8, extension_hidden: bool, alias: bool) {
        let mut info = [0u8; 32];
        let mut flags = (label & 0x07) << 1;
        if extension_hidden {
            flags |= FINDER_FLAG_EXTENSION_HIDDEN as u8;
        }
        info[9] = flags;
        if alias {
            info[8] |= (FINDER_FLAG_ALIAS >> 8) as u8;
        }
        let _ = xattr::set(path, FINDER_INFO_XATTR, &info);
    }

    #[cfg(unix)]
    fn make_symlink(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[cfg(not(unix))]
    fn make_symlink(_target: &Path, link: &Path) {
        fs::write(link, "link").unwrap();
    }

    fn unique_temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gfm-finder-metadata-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst),
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
