use crate::content::MmapContentArchive;
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
        let archives = paths
            .into_iter()
            .map(MmapContentArchive::open)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { archives })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::write_content_postings;
    use gfm_types::{ContentPositions, ContentPosting, VolumeId};
    use std::path::PathBuf;
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
