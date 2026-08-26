use crate::content::{
    content_format_error, read_content_segment, read_file_ids, read_varint, write_content_postings,
    CONTENT_SEGMENT_MAGIC, CONTENT_SEGMENT_MAGIC_V2,
};
use gfm_types::{ContentPositions, ContentPosting, FileId, GfmError, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

const DEFAULT_HOT_SEGMENT_BYTES: u64 = 1 << 20;
const DEFAULT_WARM_SEGMENT_BYTES: u64 = 16 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContentMergeTier {
    Hot,
    Warm,
    Cold,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentMergePolicy {
    pub min_merge_segments: usize,
    pub max_merge_segments: usize,
    pub max_merge_bytes: u64,
    pub hot_segment_bytes: u64,
    pub warm_segment_bytes: u64,
}

impl Default for ContentMergePolicy {
    fn default() -> Self {
        Self {
            min_merge_segments: 4,
            max_merge_segments: 16,
            max_merge_bytes: 64 << 20,
            hot_segment_bytes: DEFAULT_HOT_SEGMENT_BYTES,
            warm_segment_bytes: DEFAULT_WARM_SEGMENT_BYTES,
        }
    }
}

impl ContentMergePolicy {
    fn tier_for_bytes(&self, bytes: u64) -> ContentMergeTier {
        if bytes <= self.hot_segment_bytes {
            ContentMergeTier::Hot
        } else if bytes <= self.warm_segment_bytes {
            ContentMergeTier::Warm
        } else {
            ContentMergeTier::Cold
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSegmentSummary {
    pub path: PathBuf,
    pub bytes: u64,
    pub tombstones: usize,
    pub postings: usize,
    pub tier: ContentMergeTier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentMergePlan {
    pub merge_segments: Vec<PathBuf>,
    pub retained_segments: Vec<PathBuf>,
    pub merge_bytes: u64,
    pub tombstone_segments: usize,
    pub tier: ContentMergeTier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentMergeOutcome {
    pub postings: Vec<ContentPosting>,
    pub merged_segments: Vec<PathBuf>,
    pub retained_segments: Vec<PathBuf>,
    pub merge_bytes: u64,
    pub tombstone_segments: usize,
    pub tier: ContentMergeTier,
}

fn smallest_mergeable_tier(
    summaries: &[ContentSegmentSummary],
    policy: &ContentMergePolicy,
) -> Option<ContentMergeTier> {
    [
        ContentMergeTier::Hot,
        ContentMergeTier::Warm,
        ContentMergeTier::Cold,
    ]
    .into_iter()
    .find(|tier| {
        summaries
            .iter()
            .filter(|summary| summary.tier == *tier)
            .count()
            >= policy.min_merge_segments
    })
}

pub fn summarize_content_segment(
    path: impl AsRef<Path>,
    policy: &ContentMergePolicy,
) -> Result<ContentSegmentSummary> {
    let path = path.as_ref();
    let bytes = path
        .metadata()
        .map_err(|err| GfmError::io(path, err))?
        .len();
    let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    let mut magic = vec![0; CONTENT_SEGMENT_MAGIC.len()];
    file.read_exact(&mut magic)
        .map_err(|err| GfmError::io(path, err))?;
    if magic != CONTENT_SEGMENT_MAGIC_V2 && magic != CONTENT_SEGMENT_MAGIC {
        return Err(GfmError::Format(format!(
            "unsupported content segment header in {}",
            path.display()
        )));
    }
    let tombstones = read_file_ids(&mut file, path)?.len();
    let postings = usize::try_from(read_varint(&mut file).map_err(|err| GfmError::io(path, err))?)
        .map_err(|_| content_format_error(path, "content segment posting count overflow"))?;
    Ok(ContentSegmentSummary {
        path: path.to_path_buf(),
        bytes,
        tombstones,
        postings,
        tier: policy.tier_for_bytes(bytes),
    })
}

pub fn plan_content_segment_merge(
    segments: &[impl AsRef<Path>],
    policy: &ContentMergePolicy,
) -> Result<ContentMergePlan> {
    let mut summaries = segments
        .iter()
        .map(|path| summarize_content_segment(path.as_ref(), policy))
        .collect::<Result<Vec<_>>>()?;
    summaries.sort_by(|left, right| {
        (left.tombstones == 0)
            .cmp(&(right.tombstones == 0))
            .then_with(|| left.tier.cmp(&right.tier))
            .then_with(|| left.bytes.cmp(&right.bytes))
            .then_with(|| left.path.cmp(&right.path))
    });

    let preferred_tier = summaries
        .iter()
        .find(|summary| summary.tombstones > 0)
        .map(|summary| summary.tier)
        .or_else(|| smallest_mergeable_tier(&summaries, policy))
        .unwrap_or(ContentMergeTier::Hot);

    let mut selected_paths = BTreeSet::new();
    let mut selected_count = 0usize;
    let mut merge_bytes = 0u64;
    let mut tombstone_segments = 0usize;
    for summary in summaries
        .iter()
        .filter(|summary| summary.tombstones > 0 || summary.tier == preferred_tier)
    {
        if selected_count >= policy.max_merge_segments {
            break;
        }
        if selected_count > 0
            && merge_bytes.saturating_add(summary.bytes) > policy.max_merge_bytes
            && summary.tombstones == 0
        {
            continue;
        }
        merge_bytes = merge_bytes.saturating_add(summary.bytes);
        tombstone_segments += usize::from(summary.tombstones > 0);
        selected_paths.insert(summary.path.clone());
        selected_count += 1;
    }

    if tombstone_segments == 0 && selected_count < policy.min_merge_segments {
        selected_paths.clear();
        merge_bytes = 0;
    }

    let merge_segments = segments
        .iter()
        .filter_map(|path| {
            let path = path.as_ref().to_path_buf();
            selected_paths.contains(&path).then_some(path)
        })
        .collect();
    let retained_segments = segments
        .iter()
        .filter_map(|path| {
            let path = path.as_ref().to_path_buf();
            (!selected_paths.contains(&path)).then_some(path)
        })
        .collect();
    Ok(ContentMergePlan {
        merge_segments,
        retained_segments,
        merge_bytes,
        tombstone_segments,
        tier: preferred_tier,
    })
}

pub fn compact_content_segments_with_policy(
    output: impl AsRef<Path>,
    segments: &[impl AsRef<Path>],
    policy: &ContentMergePolicy,
) -> Result<ContentMergeOutcome> {
    let plan = plan_content_segment_merge(segments, policy)?;
    let postings = if plan.merge_segments.is_empty() {
        Vec::new()
    } else {
        compact_content_segments(output, &plan.merge_segments)?
    };
    Ok(ContentMergeOutcome {
        postings,
        merged_segments: plan.merge_segments,
        retained_segments: plan.retained_segments,
        merge_bytes: plan.merge_bytes,
        tombstone_segments: plan.tombstone_segments,
        tier: plan.tier,
    })
}

pub fn compact_content_segments(
    output: impl AsRef<Path>,
    segments: &[impl AsRef<Path>],
) -> Result<Vec<ContentPosting>> {
    compact_content_postings_with_segments(output, std::iter::empty::<ContentPosting>(), segments)
}

pub fn compact_content_postings_with_segments(
    output: impl AsRef<Path>,
    base_postings: impl IntoIterator<Item = ContentPosting>,
    segments: &[impl AsRef<Path>],
) -> Result<Vec<ContentPosting>> {
    let mut terms = content_terms_from_postings(base_postings);
    for segment_path in segments {
        let segment = read_content_segment(segment_path.as_ref())?;
        apply_content_segment(&mut terms, segment);
    }

    let postings = content_postings_from_terms(terms);
    write_content_postings(output, &postings)?;
    Ok(postings)
}

fn content_terms_from_postings(
    postings: impl IntoIterator<Item = ContentPosting>,
) -> BTreeMap<String, BTreeMap<FileId, BTreeSet<u32>>> {
    let mut terms: BTreeMap<String, BTreeMap<FileId, BTreeSet<u32>>> = BTreeMap::new();
    for posting in postings {
        merge_content_posting(&mut terms, posting);
    }
    terms
}

fn apply_content_segment(
    terms: &mut BTreeMap<String, BTreeMap<FileId, BTreeSet<u32>>>,
    segment: gfm_types::ContentSegment,
) {
    for id in segment.tombstones {
        for positions in terms.values_mut() {
            positions.remove(&id);
        }
        terms.retain(|_, positions| !positions.is_empty());
    }
    for posting in segment.postings {
        merge_content_posting(terms, posting);
    }
}

fn merge_content_posting(
    terms: &mut BTreeMap<String, BTreeMap<FileId, BTreeSet<u32>>>,
    posting: ContentPosting,
) {
    let term = posting.term.trim().to_lowercase();
    if term.is_empty() {
        return;
    }
    let ids = terms.entry(term).or_default();
    for id in posting.ids {
        ids.entry(id).or_default();
    }
    for positions in posting.positions {
        ids.entry(positions.id)
            .or_default()
            .extend(positions.positions);
    }
}

fn content_postings_from_terms(
    terms: BTreeMap<String, BTreeMap<FileId, BTreeSet<u32>>>,
) -> Vec<ContentPosting> {
    terms
        .into_iter()
        .map(|(term, positions)| ContentPosting {
            term,
            ids: positions.keys().copied().collect(),
            positions: positions
                .into_iter()
                .filter(|(_, positions)| !positions.is_empty())
                .map(|(id, positions)| ContentPositions {
                    id,
                    positions: positions.into_iter().collect(),
                })
                .collect(),
        })
        .collect()
}
