use super::*;
use crate::contentmerge::{
    compact_content_segments, compact_content_segments_checked,
    compact_content_segments_with_policy, ContentMergePolicy,
};
use gfm_types::{ContentPositions, GfmError, VolumeId};
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
fn mmap_content_archive_checked_open_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-content-open-cancel", "gfmcontent");

    let result = MmapContentArchive::open_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn checked_content_postings_read_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-content-read-cancel", "gfmcontent");

    let result = read_content_postings_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn checked_content_postings_read_can_cancel_during_checksum_load() {
    let path = temp_path("gfm-content-checksum-cancel", "gfmcontent");
    let posting = ContentPosting {
        term: "alpha".to_string(),
        ids: (0..40_000)
            .map(|node| FileId::new(VolumeId(4), node))
            .collect(),
        positions: Vec::new(),
    };
    write_content_postings(&path, &[posting]).unwrap();
    let mut checks = 0usize;

    let result = read_content_postings_checked(&path, || {
        checks += 1;
        if checks >= 8 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(checks >= 8);
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
fn content_archive_checked_open_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-content-archive-open-cancel", "gfmcontent");

    let result = ContentArchive::open_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
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
fn mmap_content_archive_checked_term_limit_honors_pre_cancelled_control() {
    let path = temp_path("gfm-content-term-limit-cancel", "gfmcontent");
    write_content_postings(
        &path,
        &[ContentPosting {
            term: "alpha".to_string(),
            ids: vec![FileId::new(VolumeId(8), 30_000)],
            positions: Vec::new(),
        }],
    )
    .unwrap();
    let archive = MmapContentArchive::open(&path).unwrap();

    let result = archive.posting_for_term_limit_checked("alpha", 1, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_content_archive_checked_term_limit_can_cancel_during_term_canonicalization() {
    let path = temp_path("gfm-content-term-canonical-cancel", "gfmcontent");
    write_content_postings(
        &path,
        &[ContentPosting {
            term: "alpha".to_string(),
            ids: vec![FileId::new(VolumeId(8), 30_001)],
            positions: Vec::new(),
        }],
    )
    .unwrap();
    let archive = MmapContentArchive::open(&path).unwrap();
    let long_term = "A".repeat(1_024);
    let mut checks = 0usize;

    let result = archive.posting_for_term_limit_checked(&long_term, 1, || {
        checks += 1;
        if checks >= 3 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(checks >= 3);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mmap_content_archive_reads_bounded_sorted_terms_for_one_volume() {
    let path = temp_path("gfm-content-volume-batch-postings", "gfmcontent");
    let volume_one_ids = (0..4)
        .map(|node| FileId::new(VolumeId(1), 10_000 + node))
        .collect::<Vec<_>>();
    let volume_two_ids = (0..5)
        .map(|node| FileId::new(VolumeId(2), 20_000 + node))
        .collect::<Vec<_>>();
    let postings = vec![
        ContentPosting {
            term: "alpha".to_string(),
            ids: volume_one_ids
                .iter()
                .chain(volume_two_ids.iter())
                .copied()
                .collect(),
            positions: volume_one_ids
                .iter()
                .chain(volume_two_ids.iter())
                .map(|id| ContentPositions {
                    id: *id,
                    positions: vec![2],
                })
                .collect(),
        },
        ContentPosting {
            term: "beta".to_string(),
            ids: volume_one_ids.clone(),
            positions: volume_one_ids
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
        .postings_for_sorted_terms_volume_limit(
            ["alpha", "alpha", "beta", "missing"],
            VolumeId(2),
            3,
        )
        .unwrap();

    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].posting.term, "alpha");
    assert_eq!(batch[0].posting.ids, volume_two_ids[..3]);
    assert_eq!(batch[0].posting.positions.len(), 3);
    assert!(batch[0].truncated);
    assert!(archive
        .postings_for_sorted_terms_volume_limit(["beta", "alpha"], VolumeId(2), 3)
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
fn checked_content_segment_read_honors_pre_cancelled_control_before_file_open() {
    let path = temp_path("gfm-content-segment-read-cancel", "gfmseg");

    let result = read_content_segment_checked(&path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
}

#[test]
fn checked_content_postings_write_preserves_existing_file_when_cancelled_before_publish() {
    let path = temp_path("gfm-content-postings-write-cancel", "gfmcontent");
    let id = FileId::new(VolumeId(4), 12);
    let original = vec![ContentPosting {
        term: "stable".to_string(),
        ids: vec![id],
        positions: vec![ContentPositions {
            id,
            positions: vec![1],
        }],
    }];
    let replacement = vec![ContentPosting {
        term: "replacement".to_string(),
        ids: vec![id],
        positions: vec![ContentPositions {
            id,
            positions: vec![2],
        }],
    }];
    write_content_postings(&path, &original).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut checks = 0usize;

    let result = write_content_postings_checked(&path, &replacement, || {
        checks += 1;
        if checks >= 5 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(checks >= 5);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(read_content_postings(&path).unwrap(), original);
    assert!(!has_atomic_temp_file(&path));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checked_content_segment_write_preserves_existing_file_when_cancelled_before_publish() {
    let path = temp_path("gfm-content-segment-write-cancel", "gfmseg");
    let id = FileId::new(VolumeId(4), 12);
    let original = ContentSegment {
        tombstones: Vec::new(),
        postings: vec![ContentPosting {
            term: "stable".to_string(),
            ids: vec![id],
            positions: vec![ContentPositions {
                id,
                positions: vec![1],
            }],
        }],
    };
    let replacement = ContentSegment {
        tombstones: vec![id],
        postings: vec![ContentPosting {
            term: "replacement".to_string(),
            ids: vec![id],
            positions: vec![ContentPositions {
                id,
                positions: vec![2],
            }],
        }],
    };
    write_content_segment(&path, &original).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut checks = 0usize;

    let result = write_content_segment_checked(&path, &replacement, || {
        checks += 1;
        if checks >= 5 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(checks >= 5);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(read_content_segment(&path).unwrap(), original);
    assert!(!has_atomic_temp_file(&path));
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
fn cancellable_compaction_stops_before_writing_output() {
    let output = temp_path("gfm-content-cancel-compact", "gfmcontent");
    let first = temp_path("gfm-content-cancel-first", "gfmseg");
    let id = FileId::new(VolumeId(3), 10);
    write_content_segment(
        &first,
        &ContentSegment {
            tombstones: Vec::new(),
            postings: vec![ContentPosting {
                term: "cancelcompact".to_string(),
                ids: vec![id],
                positions: vec![ContentPositions {
                    id,
                    positions: vec![1],
                }],
            }],
        },
    )
    .unwrap();

    let result = compact_content_segments_checked(&output, &[&first], || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!output.exists());

    std::fs::remove_file(first).unwrap();
}

#[test]
fn cancellable_compaction_stops_before_segment_file_open() {
    let output = temp_path("gfm-content-cancel-before-segment", "gfmcontent");
    let first = temp_path("gfm-content-missing-cancel-segment", "gfmseg");
    let mut checks = 0usize;

    let result = compact_content_segments_checked(&output, &[&first], || {
        checks += 1;
        if checks >= 2 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!first.exists());
    assert!(!output.exists());
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

fn has_atomic_temp_file(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let prefix = format!(".{file_name}.{}.", std::process::id());
    std::fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
        })
}
