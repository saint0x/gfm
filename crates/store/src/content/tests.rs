use super::*;
use crate::contentmerge::{
    compact_content_segments, compact_content_segments_with_policy, ContentMergePolicy,
};
use gfm_types::{ContentPositions, VolumeId};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn round_trips_content_postings() {
    let path = temp_path("gfm-content-store", "idx");
    let postings = vec![
        ContentPosting {
            term: "alpha".to_string(),
            ids: vec![FileId::new(VolumeId(4), 12), FileId::new(VolumeId(4), 15)],
            positions: vec![
                ContentPositions {
                    id: FileId::new(VolumeId(4), 12),
                    positions: vec![1, 3],
                },
                ContentPositions {
                    id: FileId::new(VolumeId(4), 15),
                    positions: vec![2],
                },
            ],
        },
        ContentPosting {
            term: "beta".to_string(),
            ids: vec![FileId::new(VolumeId(5), 3)],
            positions: vec![ContentPositions {
                id: FileId::new(VolumeId(5), 3),
                positions: vec![0],
            }],
        },
    ];

    write_content_postings(&path, &postings).unwrap();
    let read = read_content_postings(&path).unwrap();
    let archive = MmapContentArchive::open(&path).unwrap();

    assert_eq!(read, postings);
    assert_eq!(archive.postings().unwrap(), postings);
    assert_eq!(
        archive
            .postings_for_terms(["beta", "missing", "alpha", "alpha"])
            .unwrap(),
        postings
    );
    assert!(archive.postings_for_terms(["missing"]).unwrap().is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn content_archive_reads_one_term_from_directory() {
    let path = temp_path("gfm-content-archive", "gfmcontent");
    let alpha = FileId::new(VolumeId(4), 12);
    let beta = FileId::new(VolumeId(4), 15);
    write_content_postings(
        &path,
        &[
            ContentPosting {
                term: "alpha".to_string(),
                ids: vec![alpha],
                positions: Vec::new(),
            },
            ContentPosting {
                term: "beta".to_string(),
                ids: vec![beta],
                positions: Vec::new(),
            },
        ],
    )
    .unwrap();

    let mut archive = ContentArchive::open(&path).unwrap();

    assert_eq!(archive.indexed_terms(), 2);
    assert_eq!(archive.ids_for_term("beta").unwrap(), vec![beta]);
    assert!(archive.ids_for_term("missing").unwrap().is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_content_archive_reads_terms_without_file_seeks() {
    let path = temp_path("gfm-content-mmap-archive", "gfmcontent");
    let alpha = FileId::new(VolumeId(4), 12);
    let beta = FileId::new(VolumeId(4), 15);
    write_content_postings(
        &path,
        &[
            ContentPosting {
                term: "alpha".to_string(),
                ids: vec![alpha],
                positions: vec![ContentPositions {
                    id: alpha,
                    positions: vec![1, 2],
                }],
            },
            ContentPosting {
                term: "beta".to_string(),
                ids: vec![beta],
                positions: vec![ContentPositions {
                    id: beta,
                    positions: vec![8],
                }],
            },
        ],
    )
    .unwrap();

    let archive = MmapContentArchive::open(&path).unwrap();
    let posting = archive.posting_for_term("beta").unwrap().unwrap();

    assert_eq!(archive.indexed_terms(), 2);
    assert!(archive.mapped_len() > 0);
    assert_eq!(archive.ids_for_term("beta").unwrap(), vec![beta]);
    assert_eq!(posting.positions[0].positions, vec![8]);
    assert!(archive.ids_for_term("missing").unwrap().is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_content_archive_reads_one_compressed_id_block() {
    let path = temp_path("gfm-content-blocked-archive", "gfmcontent");
    let ids = (0..300)
        .map(|node| FileId::new(VolumeId(8), 20_000 + node))
        .collect::<Vec<_>>();
    let posting = ContentPosting {
        term: "needle".to_string(),
        ids: ids.clone(),
        positions: ids
            .iter()
            .map(|id| ContentPositions {
                id: *id,
                positions: vec![1, 3],
            })
            .collect(),
    };

    write_content_postings(&path, &[posting]).unwrap();
    let archive = MmapContentArchive::open(&path).unwrap();
    let block = archive.id_block_for_term("needle", 1).unwrap();
    let full = archive.posting_for_term("needle").unwrap().unwrap();

    assert_eq!(full.ids, ids);
    assert_eq!(full.positions.len(), 300);
    assert_eq!(block.len(), 128);
    assert_eq!(block[0], FileId::new(VolumeId(8), 20_128));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_content_archive_reads_bounded_compressed_posting() {
    let path = temp_path("gfm-content-bounded-posting", "gfmcontent");
    let ids = (0..300)
        .map(|node| FileId::new(VolumeId(8), 20_000 + node))
        .collect::<Vec<_>>();
    let posting = ContentPosting {
        term: "needle".to_string(),
        ids: ids.clone(),
        positions: ids
            .iter()
            .map(|id| ContentPositions {
                id: *id,
                positions: vec![1, 3],
            })
            .collect(),
    };

    write_content_postings(&path, &[posting]).unwrap();
    let archive = MmapContentArchive::open(&path).unwrap();
    let (bounded, truncated) = archive.posting_for_term_limit("needle", 3).unwrap();
    let bounded = bounded.unwrap();

    assert!(truncated);
    assert_eq!(bounded.ids, ids[..3]);
    assert_eq!(bounded.positions.len(), 3);
    assert_eq!(bounded.positions[0].positions, vec![1, 3]);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_content_archive_reads_bounded_sorted_terms_in_one_pass() {
    let path = temp_path("gfm-content-batch-postings", "gfmcontent");
    let alpha_ids = (0..4)
        .map(|node| FileId::new(VolumeId(8), 30_000 + node))
        .collect::<Vec<_>>();
    let beta_ids = (0..2)
        .map(|node| FileId::new(VolumeId(8), 40_000 + node))
        .collect::<Vec<_>>();
    let postings = vec![
        ContentPosting {
            term: "alpha".to_string(),
            ids: alpha_ids.clone(),
            positions: alpha_ids
                .iter()
                .map(|id| ContentPositions {
                    id: *id,
                    positions: vec![2],
                })
                .collect(),
        },
        ContentPosting {
            term: "beta".to_string(),
            ids: beta_ids.clone(),
            positions: beta_ids
                .iter()
                .map(|id| ContentPositions {
                    id: *id,
                    positions: vec![4],
                })
                .collect(),
        },
    ];

    write_content_postings(&path, &postings).unwrap();
    let archive = MmapContentArchive::open(&path).unwrap();
    let batch = archive
        .postings_for_sorted_terms_limit(["alpha", "alpha", "beta", "missing"], 2)
        .unwrap();

    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0].posting.term, "alpha");
    assert_eq!(batch[0].posting.ids, alpha_ids[..2]);
    assert_eq!(batch[0].posting.positions.len(), 2);
    assert!(batch[0].truncated);
    assert_eq!(batch[1].posting.term, "beta");
    assert_eq!(batch[1].posting.ids, beta_ids);
    assert!(!batch[1].truncated);
    assert!(archive
        .postings_for_sorted_terms_limit(["beta", "alpha"], 2)
        .is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checksummed_content_archive_rejects_corruption() {
    let path = temp_path("gfm-content-checksum", "gfmcontent");
    write_content_postings(
        &path,
        &[ContentPosting {
            term: "needle".to_string(),
            ids: vec![FileId::new(VolumeId(8), 20_000)],
            positions: Vec::new(),
        }],
    )
    .unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    let offset = bytes
        .windows(b"needle".len())
        .position(|window| window == b"needle")
        .expect("archive should contain the test term");
    bytes[offset] = b'z';
    std::fs::write(&path, bytes).unwrap();

    let read_error = read_content_postings(&path).unwrap_err().to_string();
    let mmap_error = MmapContentArchive::open(&path).unwrap_err().to_string();

    assert!(read_error.contains("checksum mismatch"), "{read_error}");
    assert!(mmap_error.contains("checksum mismatch"), "{mmap_error}");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn round_trips_content_segments() {
    let path = temp_path("gfm-content-segment", "gfmseg");
    let segment = ContentSegment {
        tombstones: vec![FileId::new(VolumeId(4), 12)],
        postings: vec![ContentPosting {
            term: "alpha".to_string(),
            ids: vec![FileId::new(VolumeId(4), 15)],
            positions: vec![ContentPositions {
                id: FileId::new(VolumeId(4), 15),
                positions: vec![8],
            }],
        }],
    };

    write_content_segment(&path, &segment).unwrap();
    let read = read_content_segment(&path).unwrap();

    assert_eq!(read, segment);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn compacts_content_segments_with_tombstones() {
    let first = temp_path("gfm-content-segment-first", "gfmseg");
    let second = temp_path("gfm-content-segment-second", "gfmseg");
    let output = temp_path("gfm-content-compact", "gfmcontent");
    let old = FileId::new(VolumeId(7), 10);
    let new = FileId::new(VolumeId(7), 11);

    write_content_segment(
        &first,
        &ContentSegment {
            tombstones: Vec::new(),
            postings: vec![
                ContentPosting {
                    term: "needle".to_string(),
                    ids: vec![old],
                    positions: vec![ContentPositions {
                        id: old,
                        positions: vec![1],
                    }],
                },
                ContentPosting {
                    term: "stable".to_string(),
                    ids: vec![new],
                    positions: vec![ContentPositions {
                        id: new,
                        positions: vec![2],
                    }],
                },
            ],
        },
    )
    .unwrap();
    write_content_segment(
        &second,
        &ContentSegment {
            tombstones: vec![old],
            postings: vec![ContentPosting {
                term: "needle".to_string(),
                ids: vec![new],
                positions: vec![ContentPositions {
                    id: new,
                    positions: vec![3],
                }],
            }],
        },
    )
    .unwrap();

    let compacted = compact_content_segments(&output, &[&first, &second]).unwrap();
    let reloaded = read_content_postings(&output).unwrap();

    assert_eq!(compacted, reloaded);
    assert!(reloaded
        .iter()
        .any(|posting| posting.term == "needle" && posting.ids == vec![new]));
    assert!(reloaded
        .iter()
        .any(|posting| posting.term == "stable" && posting.ids == vec![new]));
    assert!(!reloaded.iter().any(|posting| posting.ids.contains(&old)));

    std::fs::remove_file(first).unwrap();
    std::fs::remove_file(second).unwrap();
    std::fs::remove_file(output).unwrap();
}

#[test]
fn tiered_compaction_selects_bounded_merge_set_and_retains_segments() {
    let output = temp_path("gfm-content-tiered-compact", "gfmcontent");
    let first = temp_path("gfm-content-tiered-first", "gfmseg");
    let second = temp_path("gfm-content-tiered-second", "gfmseg");
    let third = temp_path("gfm-content-tiered-third", "gfmseg");
    let fourth = temp_path("gfm-content-tiered-fourth", "gfmseg");
    let old = FileId::new(VolumeId(3), 10);
    let new = FileId::new(VolumeId(3), 11);
    for (path, term, id, tombstones) in [
        (&first, "alpha", old, Vec::new()),
        (&second, "beta", new, vec![old]),
        (&third, "gamma", new, Vec::new()),
        (&fourth, "delta", new, Vec::new()),
    ] {
        write_content_segment(
            path,
            &ContentSegment {
                tombstones,
                postings: vec![ContentPosting {
                    term: term.to_string(),
                    ids: vec![id],
                    positions: vec![ContentPositions {
                        id,
                        positions: vec![1],
                    }],
                }],
            },
        )
        .unwrap();
    }

    let policy = ContentMergePolicy {
        min_merge_segments: 3,
        max_merge_segments: 3,
        max_merge_bytes: u64::MAX,
        hot_segment_bytes: u64::MAX,
        warm_segment_bytes: u64::MAX,
    };
    let outcome =
        compact_content_segments_with_policy(&output, &[&first, &second, &third, &fourth], &policy)
            .unwrap();

    assert_eq!(outcome.merged_segments.len(), 3);
    assert_eq!(outcome.retained_segments.len(), 1);
    assert_eq!(outcome.tombstone_segments, 1);
    assert!(outcome
        .postings
        .iter()
        .all(|posting| !posting.ids.contains(&old)));
    assert!(!outcome.retained_segments.contains(&second));

    std::fs::remove_file(first).unwrap();
    std::fs::remove_file(second).unwrap();
    std::fs::remove_file(third).unwrap();
    std::fs::remove_file(fourth).unwrap();
    std::fs::remove_file(output).unwrap();
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
