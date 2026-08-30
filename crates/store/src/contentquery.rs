use crate::content::MmapContentArchive;
use crate::contentset::ContentArchiveManifest;
use gfm_types::{ContentPositions, ContentPosting, FileId, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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

        let mut by_term: BTreeMap<String, BTreeMap<FileId, BTreeSet<u32>>> = selected
            .iter()
            .cloned()
            .map(|term| (term, BTreeMap::new()))
            .collect();

        for archive in &self.archives {
            for limited in archive.postings_for_sorted_terms_limit(&selected, limit_per_term)? {
                let Some(positions_by_id) = by_term.get_mut(&limited.posting.term) else {
                    continue;
                };
                merge_content_posting_positions(limited.posting, positions_by_id);
            }
        }

        Ok(by_term
            .into_iter()
            .filter_map(|(term, mut positions_by_id)| {
                if positions_by_id.is_empty() {
                    return None;
                }
                if positions_by_id.len() > limit_per_term {
                    while positions_by_id.len() > limit_per_term {
                        positions_by_id.pop_last();
                    }
                }
                Some(content_posting_from_positions(term, positions_by_id))
            })
            .collect())
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
