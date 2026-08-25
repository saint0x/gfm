use crate::content::{ContentMergeTier, MmapContentArchive};
use crate::durable;
use gfm_types::{ContentPositions, ContentPosting, FileId, GfmError, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

const CONTENT_MANIFEST_HEADER: &str = "gfm-content-manifest-v1";

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
pub struct ContentArchiveCleanupReport {
    pub removed_archives: Vec<PathBuf>,
    pub active_archives: Vec<PathBuf>,
    pub missing_archives: Vec<PathBuf>,
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
    let promotion = manifest.promote_archive(manifest_path, new_archive, retired_paths)?;
    promotion.manifest.write(manifest_path)?;
    Ok(promotion)
}

pub fn cleanup_inactive_content_archives(
    manifest_path: impl AsRef<Path>,
    candidates: &[impl AsRef<Path>],
) -> Result<ContentArchiveCleanupReport> {
    let manifest_path = manifest_path.as_ref();
    let manifest = ContentArchiveManifest::read(manifest_path)?;
    manifest.cleanup_inactive_archives(manifest_path, candidates)
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
