use crate::content::{ContentMergeTier, MmapContentArchive};
use crate::durable;
use gfm_types::{ContentPositions, ContentPosting, FileId, GfmError, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

const CONTENT_MANIFEST_HEADER: &str = "gfm-content-manifest-v1";
const CONTENT_PROMOTION_JOURNAL_HEADER: &str = "gfm-content-promotion-journal-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentArchiveManifestEntry {
    pub tier: ContentMergeTier,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentArchiveManifest {
    pub archives: Vec<ContentArchiveManifestEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentManifestPromotion {
    pub manifest: ContentArchiveManifest,
    pub retired_archives: Vec<PathBuf>,
    pub missing_retirements: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentManifestPromotionJournal {
    pub previous: ContentArchiveManifest,
    pub new_archive: ContentArchiveManifestEntry,
    pub retired_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentManifestPromotionRecoveryAction {
    Ready,
    CompletePromotion,
    RemoveStaleJournal,
    CannotRecover,
}

impl ContentManifestPromotionRecoveryAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::CompletePromotion => "complete-promotion",
            Self::RemoveStaleJournal => "remove-stale-journal",
            Self::CannotRecover => "cannot-recover",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentManifestPromotionRecoveryPlan {
    pub action: ContentManifestPromotionRecoveryAction,
    pub manifest_path: PathBuf,
    pub journal_path: PathBuf,
    pub detail: Option<String>,
}

impl ContentManifestPromotionRecoveryPlan {
    pub fn as_tsv(&self) -> String {
        format!(
            "content-manifest-promotion-recovery-plan\taction={}\tmanifest={}\tjournal={}\tdetail={}",
            self.action.as_str(),
            self.manifest_path.display(),
            self.journal_path.display(),
            self.detail.as_deref().unwrap_or("-")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentManifestPromotionRecovery {
    pub before: ContentManifestPromotionRecoveryPlan,
    pub after: ContentManifestPromotionRecoveryPlan,
    pub completed_promotion: bool,
    pub removed_journal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentArchiveCleanupReport {
    pub removed_archives: Vec<PathBuf>,
    pub active_archives: Vec<PathBuf>,
    pub missing_archives: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentArchiveCleanupPolicy {
    pub min_retired_archives: usize,
    pub min_retired_bytes: u64,
    pub max_cleanup_archives: usize,
}

impl Default for ContentArchiveCleanupPolicy {
    fn default() -> Self {
        Self {
            min_retired_archives: 1,
            min_retired_bytes: 0,
            max_cleanup_archives: 64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentArchiveCleanupAction {
    Skip,
    Cleanup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentArchiveCleanupPlan {
    pub action: ContentArchiveCleanupAction,
    pub cleanup_archives: Vec<PathBuf>,
    pub deferred_archives: Vec<PathBuf>,
    pub active_archives: Vec<PathBuf>,
    pub missing_archives: Vec<PathBuf>,
    pub active_bytes: u64,
    pub cleanup_bytes: u64,
    pub deferred_bytes: u64,
}

impl ContentArchiveManifest {
    pub fn new(archives: Vec<ContentArchiveManifestEntry>) -> Result<Self> {
        if archives.is_empty() {
            return Err(GfmError::Format(
                "content manifest requires at least one archive".to_string(),
            ));
        }
        Ok(Self { archives })
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = std::fs::File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mut lines = BufReader::new(file).lines();
        match lines.next() {
            Some(Ok(header)) if header == CONTENT_MANIFEST_HEADER => {}
            Some(Ok(header)) => {
                return Err(GfmError::Format(format!(
                    "unsupported content manifest header `{header}` in {}",
                    path.display()
                )))
            }
            Some(Err(err)) => return Err(GfmError::io(path, err)),
            None => {
                return Err(GfmError::Format(format!(
                    "empty content manifest {}",
                    path.display()
                )))
            }
        }

        let mut archives = Vec::new();
        for (line_index, line) in lines.enumerate() {
            let line = line.map_err(|err| GfmError::io(path, err))?;
            if line.trim().is_empty() {
                continue;
            }
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() != 3 || fields[0] != "archive" {
                return Err(GfmError::Format(format!(
                    "{} line {}: expected archive, tier, path",
                    path.display(),
                    line_index + 2
                )));
            }
            archives.push(ContentArchiveManifestEntry {
                tier: parse_tier(fields[1], path, line_index + 2)?,
                path: PathBuf::from(unescape(fields[2])?),
            });
        }
        Self::new(archives)
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if self.archives.is_empty() {
            return Err(GfmError::Format(
                "content manifest requires at least one archive".to_string(),
            ));
        }
        durable::atomic_write(path, |writer| {
            writeln!(writer, "{CONTENT_MANIFEST_HEADER}")?;
            for archive in &self.archives {
                writeln!(
                    writer,
                    "archive\t{}\t{}",
                    tier_name(archive.tier),
                    escape(&archive.path.to_string_lossy())
                )?;
            }
            Ok(())
        })
        .map(|_| ())
    }

    pub fn resolved_archive_paths(&self, manifest_path: impl AsRef<Path>) -> Vec<PathBuf> {
        self.archives
            .iter()
            .map(|entry| resolve_manifest_path(manifest_path.as_ref(), &entry.path))
            .collect()
    }

    pub fn promote_archive(
        &self,
        manifest_path: impl AsRef<Path>,
        new_archive: ContentArchiveManifestEntry,
        retired_paths: &[impl AsRef<Path>],
    ) -> Result<ContentManifestPromotion> {
        let manifest_path = manifest_path.as_ref();
        let retired_set = retired_paths
            .iter()
            .map(|path| resolve_manifest_path(manifest_path, path.as_ref()))
            .collect::<BTreeSet<_>>();
        let new_resolved = resolve_manifest_path(manifest_path, &new_archive.path);
        let mut retired_archives = Vec::new();
        let mut retained = Vec::new();
        for entry in &self.archives {
            let resolved = resolve_manifest_path(manifest_path, &entry.path);
            if retired_set.contains(&resolved) || resolved == new_resolved {
                retired_archives.push(resolved);
            } else {
                retained.push(entry.clone());
            }
        }

        let retired_archives_set = retired_archives.iter().cloned().collect::<BTreeSet<_>>();
        let missing_retirements = retired_set
            .into_iter()
            .filter(|path| !retired_archives_set.contains(path))
            .collect();
        retained.push(new_archive);
        Ok(ContentManifestPromotion {
            manifest: Self::new(retained)?,
            retired_archives,
            missing_retirements,
        })
    }

    pub fn cleanup_inactive_archives(
        &self,
        manifest_path: impl AsRef<Path>,
        candidates: &[impl AsRef<Path>],
    ) -> Result<ContentArchiveCleanupReport> {
        let manifest_path = manifest_path.as_ref();
        let active = self
            .resolved_archive_paths(manifest_path)
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut removed_archives = Vec::new();
        let mut active_archives = Vec::new();
        let mut missing_archives = Vec::new();
        for candidate in candidates {
            let path = resolve_manifest_path(manifest_path, candidate.as_ref());
            if active.contains(&path) {
                active_archives.push(path);
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => removed_archives.push(path),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    missing_archives.push(path);
                }
                Err(err) => return Err(GfmError::io(&path, err)),
            }
        }
        Ok(ContentArchiveCleanupReport {
            removed_archives,
            active_archives,
            missing_archives,
        })
    }

    pub fn plan_inactive_archive_cleanup(
        &self,
        manifest_path: impl AsRef<Path>,
        candidates: &[impl AsRef<Path>],
        policy: &ContentArchiveCleanupPolicy,
    ) -> Result<ContentArchiveCleanupPlan> {
        let manifest_path = manifest_path.as_ref();
        let active = self
            .resolved_archive_paths(manifest_path)
            .into_iter()
            .collect::<BTreeSet<_>>();
        let active_bytes =
            active.iter().try_fold(
                0u64,
                |total, path| Ok(total.saturating_add(file_len(path)?)),
            )?;
        let mut retired = BTreeMap::new();
        let mut active_archives = Vec::new();
        let mut missing_archives = Vec::new();

        for candidate in candidates {
            let path = resolve_manifest_path(manifest_path, candidate.as_ref());
            if active.contains(&path) {
                active_archives.push(path);
                continue;
            }
            match std::fs::metadata(&path) {
                Ok(metadata) => {
                    retired.insert(path, metadata.len());
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    missing_archives.push(path);
                }
                Err(err) => return Err(GfmError::io(&path, err)),
            }
        }

        let max_cleanup_archives = policy.max_cleanup_archives.max(1);
        let retired_count = retired.len();
        let retired_bytes = retired.values().copied().fold(0u64, u64::saturating_add);
        let should_cleanup = retired_count >= policy.min_retired_archives.max(1)
            || retired_bytes >= policy.min_retired_bytes;
        let selected_count = if should_cleanup {
            retired_count.min(max_cleanup_archives)
        } else {
            0
        };
        let mut cleanup_archives = Vec::new();
        let mut deferred_archives = Vec::new();
        let mut cleanup_bytes = 0u64;
        let mut deferred_bytes = 0u64;
        for (index, (path, bytes)) in retired.into_iter().enumerate() {
            if index < selected_count {
                cleanup_bytes = cleanup_bytes.saturating_add(bytes);
                cleanup_archives.push(path);
            } else {
                deferred_bytes = deferred_bytes.saturating_add(bytes);
                deferred_archives.push(path);
            }
        }

        Ok(ContentArchiveCleanupPlan {
            action: if cleanup_archives.is_empty() {
                ContentArchiveCleanupAction::Skip
            } else {
                ContentArchiveCleanupAction::Cleanup
            },
            cleanup_archives,
            deferred_archives,
            active_archives,
            missing_archives,
            active_bytes,
            cleanup_bytes,
            deferred_bytes,
        })
    }
}

impl ContentManifestPromotionJournal {
    pub fn new(
        previous: ContentArchiveManifest,
        new_archive: ContentArchiveManifestEntry,
        retired_paths: Vec<PathBuf>,
    ) -> Result<Self> {
        if previous.archives.is_empty() {
            return Err(GfmError::Format(
                "content promotion journal requires previous manifest archives".to_string(),
            ));
        }
        Ok(Self {
            previous,
            new_archive,
            retired_paths,
        })
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = std::fs::File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mut lines = BufReader::new(file).lines();
        match lines.next() {
            Some(Ok(header)) if header == CONTENT_PROMOTION_JOURNAL_HEADER => {}
            Some(Ok(header)) => {
                return Err(GfmError::Format(format!(
                    "unsupported content promotion journal header `{header}` in {}",
                    path.display()
                )))
            }
            Some(Err(err)) => return Err(GfmError::io(path, err)),
            None => {
                return Err(GfmError::Format(format!(
                    "empty content promotion journal {}",
                    path.display()
                )))
            }
        }

        let mut previous = Vec::new();
        let mut new_archive = None;
        let mut retired_paths = Vec::new();
        for (line_index, line) in lines.enumerate() {
            let line = line.map_err(|err| GfmError::io(path, err))?;
            if line.trim().is_empty() {
                continue;
            }
            let fields = line.split('\t').collect::<Vec<_>>();
            match fields.as_slice() {
                ["previous", tier, archive_path] => {
                    previous.push(ContentArchiveManifestEntry {
                        tier: parse_tier(tier, path, line_index + 2)?,
                        path: PathBuf::from(unescape(archive_path)?),
                    });
                }
                ["new", tier, archive_path] => {
                    if new_archive.is_some() {
                        return Err(GfmError::Format(format!(
                            "{} line {}: duplicate new archive",
                            path.display(),
                            line_index + 2
                        )));
                    }
                    new_archive = Some(ContentArchiveManifestEntry {
                        tier: parse_tier(tier, path, line_index + 2)?,
                        path: PathBuf::from(unescape(archive_path)?),
                    });
                }
                ["retire", archive_path] => {
                    retired_paths.push(PathBuf::from(unescape(archive_path)?));
                }
                _ => {
                    return Err(GfmError::Format(format!(
                        "{} line {}: expected previous, new, or retire entry",
                        path.display(),
                        line_index + 2
                    )))
                }
            }
        }
        Self::new(
            ContentArchiveManifest::new(previous)?,
            new_archive.ok_or_else(|| {
                GfmError::Format(format!(
                    "content promotion journal {} requires a new archive",
                    path.display()
                ))
            })?,
            retired_paths,
        )
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        durable::atomic_write(path, |writer| {
            writeln!(writer, "{CONTENT_PROMOTION_JOURNAL_HEADER}")?;
            for archive in &self.previous.archives {
                writeln!(
                    writer,
                    "previous\t{}\t{}",
                    tier_name(archive.tier),
                    escape(&archive.path.to_string_lossy())
                )?;
            }
            writeln!(
                writer,
                "new\t{}\t{}",
                tier_name(self.new_archive.tier),
                escape(&self.new_archive.path.to_string_lossy())
            )?;
            for path in &self.retired_paths {
                writeln!(writer, "retire\t{}", escape(&path.to_string_lossy()))?;
            }
            Ok(())
        })
        .map(|_| ())
    }

    fn promotion(&self, manifest_path: &Path) -> Result<ContentManifestPromotion> {
        self.previous
            .promote_archive(manifest_path, self.new_archive.clone(), &self.retired_paths)
    }
}

#[derive(Debug)]
pub struct MmapContentSet {
    archives: Vec<MmapContentArchive>,
}

impl MmapContentSet {
    pub fn open<I, P>(paths: I) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let archives = paths
            .into_iter()
            .map(MmapContentArchive::open)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { archives })
    }

    pub fn open_manifest(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let manifest = ContentArchiveManifest::read(path)?;
        Self::open(manifest.resolved_archive_paths(path))
    }

    pub fn ids_for_term(&self, term: &str) -> Result<Vec<FileId>> {
        Ok(self
            .posting_for_term(term)?
            .map(|posting| posting.ids)
            .unwrap_or_default())
    }

    pub fn posting_for_term(&self, term: &str) -> Result<Option<ContentPosting>> {
        let term = canonical_term(term);
        if term.is_empty() {
            return Ok(None);
        }
        let mut positions_by_id: BTreeMap<FileId, BTreeSet<u32>> = BTreeMap::new();
        for archive in &self.archives {
            let Some(posting) = archive.posting_for_term(&term)? else {
                continue;
            };
            for id in posting.ids {
                positions_by_id.entry(id).or_default();
            }
            for positions in posting.positions {
                positions_by_id
                    .entry(positions.id)
                    .or_default()
                    .extend(positions.positions);
            }
        }
        if positions_by_id.is_empty() {
            return Ok(None);
        }
        Ok(Some(content_posting_from_positions(term, positions_by_id)))
    }

    pub fn posting_for_term_limit(
        &self,
        term: &str,
        limit: usize,
    ) -> Result<(Option<ContentPosting>, bool)> {
        let term = canonical_term(term);
        if term.is_empty() {
            return Ok((None, false));
        }
        let mut positions_by_id: BTreeMap<FileId, BTreeSet<u32>> = BTreeMap::new();
        let mut truncated = false;
        for archive in &self.archives {
            let (Some(posting), archive_truncated) =
                archive.posting_for_term_limit(&term, limit)?
            else {
                continue;
            };
            truncated |= archive_truncated;
            for id in posting.ids {
                positions_by_id.entry(id).or_default();
            }
            for positions in posting.positions {
                positions_by_id
                    .entry(positions.id)
                    .or_default()
                    .extend(positions.positions);
            }
        }
        if positions_by_id.is_empty() {
            return Ok((None, truncated));
        }
        if positions_by_id.len() > limit {
            truncated = true;
            while positions_by_id.len() > limit {
                positions_by_id.pop_last();
            }
        }
        Ok((
            Some(content_posting_from_positions(term, positions_by_id)),
            truncated,
        ))
    }

    pub fn postings_for_terms<I, S>(&self, terms: I) -> Result<Vec<ContentPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();
        for term in terms {
            let term = canonical_term(term.as_ref());
            if !term.is_empty() {
                selected.insert(term);
            }
        }

        selected
            .into_iter()
            .filter_map(|term| self.posting_for_term(&term).transpose())
            .collect()
    }

    pub fn postings_for_terms_limit<I, S>(
        &self,
        terms: I,
        limit_per_term: usize,
    ) -> Result<Vec<ContentPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();
        for term in terms {
            let term = canonical_term(term.as_ref());
            if !term.is_empty() {
                selected.insert(term);
            }
        }

        let mut postings = Vec::new();
        for term in selected {
            if let (Some(posting), _) = self.posting_for_term_limit(&term, limit_per_term)? {
                postings.push(posting);
            }
        }
        Ok(postings)
    }

    pub fn archive_count(&self) -> usize {
        self.archives.len()
    }

    pub fn indexed_terms(&self) -> usize {
        self.archives
            .iter()
            .map(MmapContentArchive::indexed_terms)
            .sum()
    }

    pub fn mapped_len(&self) -> usize {
        self.archives
            .iter()
            .map(MmapContentArchive::mapped_len)
            .sum()
    }
}

pub fn write_content_archive_manifest(
    path: impl AsRef<Path>,
    manifest: &ContentArchiveManifest,
) -> Result<()> {
    manifest.write(path)
}

pub fn read_content_archive_manifest(path: impl AsRef<Path>) -> Result<ContentArchiveManifest> {
    ContentArchiveManifest::read(path)
}

pub fn promote_content_archive_manifest(
    manifest_path: impl AsRef<Path>,
    new_archive: ContentArchiveManifestEntry,
    retired_paths: &[impl AsRef<Path>],
) -> Result<ContentManifestPromotion> {
    let manifest_path = manifest_path.as_ref();
    let manifest = ContentArchiveManifest::read(manifest_path)?;
    let retired_paths = retired_paths
        .iter()
        .map(|path| path.as_ref().to_path_buf())
        .collect::<Vec<_>>();
    let journal = ContentManifestPromotionJournal::new(
        manifest.clone(),
        new_archive.clone(),
        retired_paths.clone(),
    )?;
    let journal_path = content_manifest_promotion_journal_path(manifest_path);
    journal.write(&journal_path)?;
    let promotion = manifest.promote_archive(manifest_path, new_archive, &retired_paths)?;
    promotion.manifest.write(manifest_path)?;
    remove_journal_if_exists(&journal_path)?;
    Ok(promotion)
}

pub fn content_manifest_promotion_journal_path(manifest_path: impl AsRef<Path>) -> PathBuf {
    let manifest_path = manifest_path.as_ref();
    let file_name = manifest_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("content.gfmmanifest");
    manifest_path.with_file_name(format!(".{file_name}.promotion-journal"))
}

pub fn plan_content_manifest_promotion_recovery(
    manifest_path: impl AsRef<Path>,
) -> ContentManifestPromotionRecoveryPlan {
    let manifest_path = manifest_path.as_ref().to_path_buf();
    let journal_path = content_manifest_promotion_journal_path(&manifest_path);
    if !journal_path.exists() {
        return ContentManifestPromotionRecoveryPlan {
            action: ContentManifestPromotionRecoveryAction::Ready,
            manifest_path,
            journal_path,
            detail: Some("no pending promotion journal".to_string()),
        };
    }
    let journal = match ContentManifestPromotionJournal::read(&journal_path) {
        Ok(journal) => journal,
        Err(err) => {
            return ContentManifestPromotionRecoveryPlan {
                action: ContentManifestPromotionRecoveryAction::CannotRecover,
                manifest_path,
                journal_path,
                detail: Some(err.to_string()),
            }
        }
    };
    let promotion = match journal.promotion(&manifest_path) {
        Ok(promotion) => promotion,
        Err(err) => {
            return ContentManifestPromotionRecoveryPlan {
                action: ContentManifestPromotionRecoveryAction::CannotRecover,
                manifest_path,
                journal_path,
                detail: Some(err.to_string()),
            }
        }
    };
    let current = ContentArchiveManifest::read(&manifest_path);
    if current
        .as_ref()
        .is_ok_and(|manifest| *manifest == promotion.manifest)
    {
        return ContentManifestPromotionRecoveryPlan {
            action: ContentManifestPromotionRecoveryAction::RemoveStaleJournal,
            manifest_path,
            journal_path,
            detail: Some("manifest already contains promoted archive".to_string()),
        };
    }
    for entry in &promotion.manifest.archives {
        let path = resolve_manifest_path(&manifest_path, &entry.path);
        if let Err(err) = MmapContentArchive::open(&path) {
            return ContentManifestPromotionRecoveryPlan {
                action: ContentManifestPromotionRecoveryAction::CannotRecover,
                manifest_path,
                journal_path,
                detail: Some(format!(
                    "promoted archive {} is not readable: {err}",
                    path.display()
                )),
            };
        }
    }
    ContentManifestPromotionRecoveryPlan {
        action: ContentManifestPromotionRecoveryAction::CompletePromotion,
        manifest_path,
        journal_path,
        detail: current.err().map(|err| err.to_string()),
    }
}

pub fn recover_content_manifest_promotion(
    manifest_path: impl AsRef<Path>,
) -> Result<ContentManifestPromotionRecovery> {
    let manifest_path = manifest_path.as_ref();
    let before = plan_content_manifest_promotion_recovery(manifest_path);
    let mut completed_promotion = false;
    let mut removed_journal = false;
    match before.action {
        ContentManifestPromotionRecoveryAction::Ready => {}
        ContentManifestPromotionRecoveryAction::RemoveStaleJournal => {
            remove_journal_if_exists(&before.journal_path)?;
            removed_journal = true;
        }
        ContentManifestPromotionRecoveryAction::CompletePromotion => {
            let journal = ContentManifestPromotionJournal::read(&before.journal_path)?;
            let promotion = journal.promotion(manifest_path)?;
            promotion.manifest.write(manifest_path)?;
            remove_journal_if_exists(&before.journal_path)?;
            completed_promotion = true;
            removed_journal = true;
        }
        ContentManifestPromotionRecoveryAction::CannotRecover => {
            return Err(GfmError::Format(format!(
                "{} promotion cannot be recovered: {}",
                manifest_path.display(),
                before
                    .detail
                    .as_deref()
                    .unwrap_or("invalid promotion journal")
            )))
        }
    }
    let after = plan_content_manifest_promotion_recovery(manifest_path);
    Ok(ContentManifestPromotionRecovery {
        before,
        after,
        completed_promotion,
        removed_journal,
    })
}

pub fn cleanup_inactive_content_archives(
    manifest_path: impl AsRef<Path>,
    candidates: &[impl AsRef<Path>],
) -> Result<ContentArchiveCleanupReport> {
    let manifest_path = manifest_path.as_ref();
    let manifest = ContentArchiveManifest::read(manifest_path)?;
    manifest.cleanup_inactive_archives(manifest_path, candidates)
}

pub fn plan_inactive_content_archive_cleanup(
    manifest_path: impl AsRef<Path>,
    candidates: &[impl AsRef<Path>],
    policy: &ContentArchiveCleanupPolicy,
) -> Result<ContentArchiveCleanupPlan> {
    let manifest_path = manifest_path.as_ref();
    let manifest = ContentArchiveManifest::read(manifest_path)?;
    manifest.plan_inactive_archive_cleanup(manifest_path, candidates, policy)
}

fn file_len(path: &Path) -> Result<u64> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(err) => Err(GfmError::io(path, err)),
    }
}

fn remove_journal_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            let _ = durable::sync_parent_for_path(path);
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(GfmError::io(path, err)),
    }
}

fn canonical_term(term: &str) -> String {
    term.trim().to_lowercase()
}

fn content_posting_from_positions(
    term: String,
    positions_by_id: BTreeMap<FileId, BTreeSet<u32>>,
) -> ContentPosting {
    ContentPosting {
        term,
        ids: positions_by_id.keys().copied().collect(),
        positions: positions_by_id
            .into_iter()
            .filter(|(_, positions)| !positions.is_empty())
            .map(|(id, positions)| ContentPositions {
                id,
                positions: positions.into_iter().collect(),
            })
            .collect(),
    }
}

fn tier_name(tier: ContentMergeTier) -> &'static str {
    match tier {
        ContentMergeTier::Hot => "hot",
        ContentMergeTier::Warm => "warm",
        ContentMergeTier::Cold => "cold",
    }
}

fn parse_tier(value: &str, path: &Path, line: usize) -> Result<ContentMergeTier> {
    match value {
        "hot" => Ok(ContentMergeTier::Hot),
        "warm" => Ok(ContentMergeTier::Warm),
        "cold" => Ok(ContentMergeTier::Cold),
        other => Err(GfmError::Format(format!(
            "{} line {line}: unsupported content archive tier `{other}`",
            path.display()
        ))),
    }
}

fn resolve_manifest_path(manifest_path: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        manifest_path
            .parent()
            .map(|parent| parent.join(path))
            .unwrap_or_else(|| path.to_path_buf())
    }
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
                    "invalid content manifest escape `\\{other}`"
                )))
            }
            None => {
                return Err(GfmError::Format(
                    "trailing content manifest escape".to_string(),
                ))
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::write_content_postings;
    use gfm_types::{ContentPositions, ContentPosting, VolumeId};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn mmap_content_set_merges_duplicate_terms_without_loading_full_archives() {
        let first = temp_path("gfm-content-set-first", "gfmcontent");
        let second = temp_path("gfm-content-set-second", "gfmcontent");
        let left = FileId::new(VolumeId(1), 10);
        let right = FileId::new(VolumeId(1), 11);

        write_content_postings(
            &first,
            &[ContentPosting {
                term: "needle".to_string(),
                ids: vec![left],
                positions: vec![ContentPositions {
                    id: left,
                    positions: vec![1, 3],
                }],
            }],
        )
        .unwrap();
        write_content_postings(
            &second,
            &[ContentPosting {
                term: "needle".to_string(),
                ids: vec![left, right],
                positions: vec![
                    ContentPositions {
                        id: left,
                        positions: vec![3, 7],
                    },
                    ContentPositions {
                        id: right,
                        positions: vec![2],
                    },
                ],
            }],
        )
        .unwrap();

        let set = MmapContentSet::open([&first, &second]).unwrap();
        let posting = set.posting_for_term("NEEDLE").unwrap().unwrap();

        assert_eq!(set.archive_count(), 2);
        assert_eq!(posting.ids, vec![left, right]);
        assert_eq!(
            posting.positions,
            vec![
                ContentPositions {
                    id: left,
                    positions: vec![1, 3, 7],
                },
                ContentPositions {
                    id: right,
                    positions: vec![2],
                }
            ]
        );

        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
    }

    #[test]
    fn mmap_content_set_reads_bounded_term_union_deterministically() {
        let first = temp_path("gfm-content-set-bounded-first", "gfmcontent");
        let second = temp_path("gfm-content-set-bounded-second", "gfmcontent");
        let high_ids = (100..200)
            .map(|node| FileId::new(VolumeId(1), node))
            .collect::<Vec<_>>();
        let low_ids = (1..100)
            .map(|node| FileId::new(VolumeId(1), node))
            .collect::<Vec<_>>();
        write_content_postings(
            &first,
            &[ContentPosting {
                term: "needle".to_string(),
                ids: high_ids.clone(),
                positions: high_ids
                    .iter()
                    .map(|id| ContentPositions {
                        id: *id,
                        positions: vec![1],
                    })
                    .collect(),
            }],
        )
        .unwrap();
        write_content_postings(
            &second,
            &[ContentPosting {
                term: "needle".to_string(),
                ids: low_ids.clone(),
                positions: low_ids
                    .iter()
                    .map(|id| ContentPositions {
                        id: *id,
                        positions: vec![2],
                    })
                    .collect(),
            }],
        )
        .unwrap();

        let set = MmapContentSet::open([&first, &second]).unwrap();
        let (posting, truncated) = set.posting_for_term_limit("needle", 3).unwrap();
        let posting = posting.unwrap();

        assert!(truncated);
        assert_eq!(
            posting.ids,
            vec![
                FileId::new(VolumeId(1), 1),
                FileId::new(VolumeId(1), 2),
                FileId::new(VolumeId(1), 3)
            ]
        );
        assert_eq!(
            posting
                .positions
                .iter()
                .map(|positions| positions.id)
                .collect::<Vec<_>>(),
            posting.ids
        );

        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
    }

    #[test]
    fn content_archive_manifest_round_trips_and_resolves_relative_paths() {
        let dir = temp_dir("gfm-content-manifest-root");
        let first = dir.join("hot.gfmcontent");
        let nested = dir.join("tier");
        let second = nested.join("warm.gfmcontent");
        let manifest_path = dir.join("content.gfmmanifest");
        std::fs::create_dir_all(&nested).unwrap();
        write_content_postings(&first, &[]).unwrap();
        write_content_postings(&second, &[]).unwrap();

        let manifest = ContentArchiveManifest::new(vec![
            ContentArchiveManifestEntry {
                tier: ContentMergeTier::Hot,
                path: PathBuf::from("hot.gfmcontent"),
            },
            ContentArchiveManifestEntry {
                tier: ContentMergeTier::Warm,
                path: PathBuf::from("tier/warm.gfmcontent"),
            },
        ])
        .unwrap();
        manifest.write(&manifest_path).unwrap();

        let reloaded = ContentArchiveManifest::read(&manifest_path).unwrap();
        assert_eq!(reloaded, manifest);
        assert_eq!(
            reloaded.resolved_archive_paths(&manifest_path),
            vec![first, second]
        );
        assert_eq!(
            MmapContentSet::open_manifest(&manifest_path)
                .unwrap()
                .archive_count(),
            2
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn content_archive_manifest_promotes_new_archive_and_reports_retirement_state() {
        let dir = temp_dir("gfm-content-manifest-promote");
        let manifest_path = dir.join("content.gfmmanifest");
        std::fs::create_dir_all(&dir).unwrap();
        write_content_postings(dir.join("warm-b.gfmcontent"), &[]).unwrap();

        let manifest = ContentArchiveManifest::new(vec![
            ContentArchiveManifestEntry {
                tier: ContentMergeTier::Hot,
                path: PathBuf::from("hot-a.gfmcontent"),
            },
            ContentArchiveManifestEntry {
                tier: ContentMergeTier::Hot,
                path: PathBuf::from("hot-b.gfmcontent"),
            },
            ContentArchiveManifestEntry {
                tier: ContentMergeTier::Warm,
                path: PathBuf::from("warm-a.gfmcontent"),
            },
        ])
        .unwrap();
        manifest.write(&manifest_path).unwrap();

        let promotion = promote_content_archive_manifest(
            &manifest_path,
            ContentArchiveManifestEntry {
                tier: ContentMergeTier::Warm,
                path: PathBuf::from("warm-b.gfmcontent"),
            },
            &[
                PathBuf::from("hot-a.gfmcontent"),
                PathBuf::from("missing.gfmcontent"),
            ],
        )
        .unwrap();

        assert_eq!(
            promotion.retired_archives,
            vec![dir.join("hot-a.gfmcontent")]
        );
        assert_eq!(
            promotion.missing_retirements,
            vec![dir.join("missing.gfmcontent")]
        );
        let reloaded = ContentArchiveManifest::read(&manifest_path).unwrap();
        assert_eq!(
            reloaded.archives,
            vec![
                ContentArchiveManifestEntry {
                    tier: ContentMergeTier::Hot,
                    path: PathBuf::from("hot-b.gfmcontent"),
                },
                ContentArchiveManifestEntry {
                    tier: ContentMergeTier::Warm,
                    path: PathBuf::from("warm-a.gfmcontent"),
                },
                ContentArchiveManifestEntry {
                    tier: ContentMergeTier::Warm,
                    path: PathBuf::from("warm-b.gfmcontent"),
                }
            ]
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn content_manifest_promotion_recovery_completes_pending_journal() {
        let dir = temp_dir("gfm-content-manifest-promotion-recovery");
        let manifest_path = dir.join("content.gfmmanifest");
        let old_archive = dir.join("hot-a.gfmcontent");
        let new_archive = dir.join("warm-b.gfmcontent");
        std::fs::create_dir_all(&dir).unwrap();
        write_content_postings(&old_archive, &[]).unwrap();
        write_content_postings(&new_archive, &[]).unwrap();
        let previous = ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path: PathBuf::from("hot-a.gfmcontent"),
        }])
        .unwrap();
        previous.write(&manifest_path).unwrap();
        let journal = ContentManifestPromotionJournal::new(
            previous,
            ContentArchiveManifestEntry {
                tier: ContentMergeTier::Warm,
                path: PathBuf::from("warm-b.gfmcontent"),
            },
            vec![PathBuf::from("hot-a.gfmcontent")],
        )
        .unwrap();
        let journal_path = content_manifest_promotion_journal_path(&manifest_path);
        journal.write(&journal_path).unwrap();

        let plan = plan_content_manifest_promotion_recovery(&manifest_path);
        assert_eq!(
            plan.action,
            ContentManifestPromotionRecoveryAction::CompletePromotion
        );

        let recovery = recover_content_manifest_promotion(&manifest_path).unwrap();

        assert!(recovery.completed_promotion);
        assert!(recovery.removed_journal);
        assert_eq!(
            recovery.after.action,
            ContentManifestPromotionRecoveryAction::Ready
        );
        assert!(!journal_path.exists());
        let recovered = ContentArchiveManifest::read(&manifest_path).unwrap();
        assert_eq!(
            recovered.archives,
            vec![ContentArchiveManifestEntry {
                tier: ContentMergeTier::Warm,
                path: PathBuf::from("warm-b.gfmcontent"),
            }]
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn content_manifest_promotion_recovery_removes_stale_journal() {
        let dir = temp_dir("gfm-content-manifest-promotion-stale");
        let manifest_path = dir.join("content.gfmmanifest");
        let old_archive = dir.join("hot-a.gfmcontent");
        let new_archive = dir.join("warm-b.gfmcontent");
        std::fs::create_dir_all(&dir).unwrap();
        write_content_postings(&old_archive, &[]).unwrap();
        write_content_postings(&new_archive, &[]).unwrap();
        let previous = ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path: PathBuf::from("hot-a.gfmcontent"),
        }])
        .unwrap();
        let journal = ContentManifestPromotionJournal::new(
            previous,
            ContentArchiveManifestEntry {
                tier: ContentMergeTier::Warm,
                path: PathBuf::from("warm-b.gfmcontent"),
            },
            vec![PathBuf::from("hot-a.gfmcontent")],
        )
        .unwrap();
        let promoted = journal.promotion(&manifest_path).unwrap();
        promoted.manifest.write(&manifest_path).unwrap();
        let journal_path = content_manifest_promotion_journal_path(&manifest_path);
        journal.write(&journal_path).unwrap();

        let recovery = recover_content_manifest_promotion(&manifest_path).unwrap();

        assert!(!recovery.completed_promotion);
        assert!(recovery.removed_journal);
        assert_eq!(
            recovery.before.action,
            ContentManifestPromotionRecoveryAction::RemoveStaleJournal
        );
        assert!(!journal_path.exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn content_archive_cleanup_removes_only_inactive_candidates() {
        let dir = temp_dir("gfm-content-manifest-cleanup");
        let manifest_path = dir.join("content.gfmmanifest");
        let inactive = dir.join("inactive.gfmcontent");
        let active = dir.join("active.gfmcontent");
        std::fs::create_dir_all(&dir).unwrap();
        write_content_postings(&inactive, &[]).unwrap();
        write_content_postings(&active, &[]).unwrap();

        ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path: PathBuf::from("active.gfmcontent"),
        }])
        .unwrap()
        .write(&manifest_path)
        .unwrap();

        let report = cleanup_inactive_content_archives(
            &manifest_path,
            &[
                PathBuf::from("inactive.gfmcontent"),
                PathBuf::from("active.gfmcontent"),
                PathBuf::from("missing.gfmcontent"),
            ],
        )
        .unwrap();

        assert_eq!(report.removed_archives, vec![inactive.clone()]);
        assert_eq!(report.active_archives, vec![active.clone()]);
        assert_eq!(
            report.missing_archives,
            vec![dir.join("missing.gfmcontent")]
        );
        assert!(!inactive.exists());
        assert!(active.exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn content_archive_cleanup_plan_batches_retired_archives() {
        let dir = temp_dir("gfm-content-manifest-cleanup-plan");
        let manifest_path = dir.join("content.gfmmanifest");
        let active = dir.join("active.gfmcontent");
        let first_retired = dir.join("a-retired.gfmcontent");
        let second_retired = dir.join("b-retired.gfmcontent");
        std::fs::create_dir_all(&dir).unwrap();
        write_content_postings(&active, &[]).unwrap();
        write_content_postings(&first_retired, &[]).unwrap();
        write_content_postings(&second_retired, &[]).unwrap();

        ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path: PathBuf::from("active.gfmcontent"),
        }])
        .unwrap()
        .write(&manifest_path)
        .unwrap();

        let manifest = ContentArchiveManifest::read(&manifest_path).unwrap();
        let skipped = manifest
            .plan_inactive_archive_cleanup(
                &manifest_path,
                &[
                    PathBuf::from("a-retired.gfmcontent"),
                    PathBuf::from("b-retired.gfmcontent"),
                    PathBuf::from("active.gfmcontent"),
                    PathBuf::from("missing.gfmcontent"),
                ],
                &ContentArchiveCleanupPolicy {
                    min_retired_archives: 3,
                    min_retired_bytes: u64::MAX,
                    max_cleanup_archives: 1,
                },
            )
            .unwrap();
        assert_eq!(skipped.action, ContentArchiveCleanupAction::Skip);
        assert!(skipped.cleanup_archives.is_empty());
        assert_eq!(skipped.deferred_archives.len(), 2);

        let scheduled = manifest
            .plan_inactive_archive_cleanup(
                &manifest_path,
                &[
                    PathBuf::from("a-retired.gfmcontent"),
                    PathBuf::from("b-retired.gfmcontent"),
                    PathBuf::from("active.gfmcontent"),
                    PathBuf::from("missing.gfmcontent"),
                ],
                &ContentArchiveCleanupPolicy {
                    min_retired_archives: 2,
                    min_retired_bytes: u64::MAX,
                    max_cleanup_archives: 1,
                },
            )
            .unwrap();
        assert_eq!(scheduled.action, ContentArchiveCleanupAction::Cleanup);
        assert_eq!(scheduled.cleanup_archives, vec![first_retired]);
        assert_eq!(scheduled.deferred_archives, vec![second_retired]);
        assert_eq!(scheduled.active_archives, vec![active]);
        assert_eq!(
            scheduled.missing_archives,
            vec![dir.join("missing.gfmcontent")]
        );
        assert!(scheduled.active_bytes > 0);
        assert!(scheduled.cleanup_bytes > 0);
        assert!(scheduled.deferred_bytes > 0);

        std::fs::remove_dir_all(dir).unwrap();
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

    fn temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{}-{}",
            prefix,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
