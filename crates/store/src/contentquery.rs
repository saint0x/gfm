use crate::content::MmapContentArchive;
use crate::contentset::ContentArchiveManifest;
use gfm_types::{ContentPositions, ContentPosting, FileId, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const CONTENT_SET_TERM_CHECK_STRIDE: usize = 256;

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
        Self::open_checked(paths, || Ok(()))
    }

    pub fn open_checked<I, P>(
        paths: I,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        check_control()?;
        let archives = paths
            .into_iter()
            .map(|path| {
                check_control()?;
                MmapContentArchive::open_checked(path, &mut check_control)
            })
            .collect::<Result<Vec<_>>>()?;
        check_control()?;
        Ok(Self { archives })
    }

    pub fn open_manifest(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_manifest_checked(path, || Ok(()))
    }

    pub fn open_manifest_checked(
        path: impl AsRef<Path>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let path = path.as_ref();
        check_control()?;
        let manifest = ContentArchiveManifest::read_checked(path, &mut check_control)?;
        check_control()?;
        Self::open_checked(manifest.resolved_archive_paths(path), check_control)
    }

    pub fn ids_for_term(&self, term: &str) -> Result<Vec<FileId>> {
        Ok(self
            .posting_for_term_checked(term, || Ok(()))?
            .map(|posting| posting.ids)
            .unwrap_or_default())
    }

    pub fn ids_for_term_checked(
        &self,
        term: &str,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<FileId>> {
        Ok(self
            .posting_for_term_checked(term, check_control)?
            .map(|posting| posting.ids)
            .unwrap_or_default())
    }

    pub fn posting_for_term(&self, term: &str) -> Result<Option<ContentPosting>> {
        self.posting_for_term_checked(term, || Ok(()))
    }

    pub fn posting_for_term_checked(
        &self,
        term: &str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Option<ContentPosting>> {
        check_control()?;
        let term = canonical_term_checked(term, &mut check_control)?;
        if term.is_empty() {
            return Ok(None);
        }
        let mut positions_by_id: BTreeMap<FileId, BTreeSet<u32>> = BTreeMap::new();
        for archive in &self.archives {
            check_control()?;
            let Some(posting) = archive.posting_for_term_checked(&term, &mut check_control)? else {
                continue;
            };
            merge_content_posting_positions(posting, &mut positions_by_id);
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
        self.posting_for_term_limit_checked(term, limit, || Ok(()))
    }

    pub fn posting_for_term_limit_checked(
        &self,
        term: &str,
        limit: usize,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<(Option<ContentPosting>, bool)> {
        check_control()?;
        let term = canonical_term_checked(term, &mut check_control)?;
        if term.is_empty() {
            return Ok((None, false));
        }
        let mut positions_by_id: BTreeMap<FileId, BTreeSet<u32>> = BTreeMap::new();
        let mut truncated = false;
        for archive in &self.archives {
            check_control()?;
            let (Some(posting), archive_truncated) =
                archive.posting_for_term_limit_checked(&term, limit, &mut check_control)?
            else {
                continue;
            };
            truncated |= archive_truncated;
            merge_content_posting_positions(posting, &mut positions_by_id);
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
        check_control()?;
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
        self.postings_for_terms_checked(terms, || Ok(()))
    }

    pub fn postings_for_terms_checked<I, S>(
        &self,
        terms: I,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ContentPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();
        for term in terms {
            check_control()?;
            let term = canonical_term_checked(term.as_ref(), &mut check_control)?;
            if !term.is_empty() {
                selected.insert(term);
            }
        }

        selected
            .into_iter()
            .filter_map(|term| {
                check_control()
                    .and_then(|_| self.posting_for_term_checked(&term, &mut check_control))
                    .transpose()
            })
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
        self.postings_for_terms_limit_checked(terms, limit_per_term, || Ok(()))
    }

    pub fn postings_for_terms_limit_checked<I, S>(
        &self,
        terms: I,
        limit_per_term: usize,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ContentPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();
        for term in terms {
            check_control()?;
            let term = canonical_term_checked(term.as_ref(), &mut check_control)?;
            if !term.is_empty() {
                selected.insert(term);
            }
        }

        let mut by_term: BTreeMap<String, BTreeMap<FileId, BTreeSet<u32>>> = selected
            .iter()
            .cloned()
            .map(|term| (term, BTreeMap::new()))
            .collect();

        for archive in &self.archives {
            check_control()?;
            for limited in archive.postings_for_sorted_terms_limit_checked(
                &selected,
                limit_per_term,
                &mut check_control,
            )? {
                check_control()?;
                let Some(positions_by_id) = by_term.get_mut(&limited.posting.term) else {
                    continue;
                };
                merge_content_posting_positions(limited.posting, positions_by_id);
            }
        }

        by_term
            .into_iter()
            .filter_map(|(term, mut positions_by_id)| {
                if let Err(err) = check_control() {
                    return Some(Err(err));
                }
                if positions_by_id.is_empty() {
                    return None;
                }
                if positions_by_id.len() > limit_per_term {
                    while positions_by_id.len() > limit_per_term {
                        positions_by_id.pop_last();
                    }
                }
                Some(Ok(content_posting_from_positions(term, positions_by_id)))
            })
            .collect::<Result<Vec<_>>>()
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

fn canonical_term_checked(
    term: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<String> {
    check_control()?;
    let mut canonical = String::new();
    for (index, ch) in term.trim().chars().enumerate() {
        if index.is_multiple_of(CONTENT_SET_TERM_CHECK_STRIDE) {
            check_control()?;
        }
        canonical.extend(ch.to_lowercase());
    }
    check_control()?;
    Ok(canonical)
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

fn merge_content_posting_positions(
    posting: ContentPosting,
    positions_by_id: &mut BTreeMap<FileId, BTreeSet<u32>>,
) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::write_content_postings;
    use gfm_types::{GfmError, VolumeId};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn content_set_checked_limit_query_honors_pre_cancelled_control() {
        let set = MmapContentSet {
            archives: Vec::new(),
        };

        let result = set.posting_for_term_limit_checked("needle", 10, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
    }

    #[test]
    fn content_set_checked_limit_query_can_cancel_during_term_canonicalization() {
        let set = MmapContentSet {
            archives: Vec::new(),
        };
        let long_term = "Needle".repeat(256);
        let mut checks = 0usize;

        let result = set.posting_for_term_limit_checked(&long_term, 10, || {
            checks += 1;
            if checks >= 3 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 3);
    }

    #[test]
    fn content_set_checked_ids_query_can_cancel_during_term_canonicalization() {
        let set = MmapContentSet {
            archives: Vec::new(),
        };
        let long_term = "Needle".repeat(256);
        let mut checks = 0usize;

        let result = set.ids_for_term_checked(&long_term, || {
            checks += 1;
            if checks >= 3 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 3);
    }

    #[test]
    fn content_set_checked_terms_query_cancels_before_archive_traversal() {
        let path = temp_path("gfm-content-set-query-cancel", "gfmcontent");
        write_content_postings(
            &path,
            &[ContentPosting {
                term: "needle".to_string(),
                ids: vec![FileId::new(VolumeId(1), 7)],
                positions: Vec::new(),
            }],
        )
        .unwrap();
        let set = MmapContentSet::open([&path]).unwrap();
        let mut checks = 0usize;

        let result = set.postings_for_terms_limit_checked(["needle"], 10, || {
            checks += 1;
            if checks >= 2 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 2);
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
