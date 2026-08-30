use super::*;
use crate::content::write_content_postings;
use crate::contentquery::MmapContentSet;
use gfm_types::{ContentPositions, ContentPosting, FileId, GfmError, VolumeId};
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
fn mmap_content_set_batches_selected_terms_across_archives() {
    let first = temp_path("gfm-content-set-batch-first", "gfmcontent");
    let second = temp_path("gfm-content-set-batch-second", "gfmcontent");
    let alpha_left = FileId::new(VolumeId(2), 10);
    let alpha_right = FileId::new(VolumeId(2), 11);
    let beta = FileId::new(VolumeId(2), 20);

    write_content_postings(
        &first,
        &[
            ContentPosting {
                term: "alpha".to_string(),
                ids: vec![alpha_left],
                positions: vec![ContentPositions {
                    id: alpha_left,
                    positions: vec![1],
                }],
            },
            ContentPosting {
                term: "beta".to_string(),
                ids: vec![beta],
                positions: vec![ContentPositions {
                    id: beta,
                    positions: vec![5],
                }],
            },
        ],
    )
    .unwrap();
    write_content_postings(
        &second,
        &[ContentPosting {
            term: "alpha".to_string(),
            ids: vec![alpha_left, alpha_right],
            positions: vec![
                ContentPositions {
                    id: alpha_left,
                    positions: vec![3],
                },
                ContentPositions {
                    id: alpha_right,
                    positions: vec![7],
                },
            ],
        }],
    )
    .unwrap();

    let set = MmapContentSet::open([&first, &second]).unwrap();
    let postings = set
        .postings_for_terms_limit(["missing", "beta", "alpha", "alpha"], 4)
        .unwrap();

    assert_eq!(postings.len(), 2);
    assert_eq!(postings[0].term, "alpha");
    assert_eq!(postings[0].ids, vec![alpha_left, alpha_right]);
    assert_eq!(
        postings[0].positions,
        vec![
            ContentPositions {
                id: alpha_left,
                positions: vec![1, 3],
            },
            ContentPositions {
                id: alpha_right,
                positions: vec![7],
            },
        ]
    );
    assert_eq!(postings[1].term, "beta");
    assert_eq!(postings[1].ids, vec![beta]);

    std::fs::remove_file(first).unwrap();
    std::fs::remove_file(second).unwrap();
}

#[test]
fn mmap_content_set_checked_open_honors_pre_cancelled_control_before_archive_open() {
    let path = temp_path("gfm-content-set-open-cancel", "gfmcontent");

    let result = MmapContentSet::open_checked([&path], || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!path.exists());
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
fn content_archive_manifest_checked_read_honors_pre_cancelled_control_before_file_open() {
    let dir = temp_dir("gfm-content-manifest-read-cancel");
    let manifest_path = dir.join("content.gfmmanifest");
    std::fs::create_dir_all(&dir).unwrap();

    let result = ContentArchiveManifest::read_checked(&manifest_path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!manifest_path.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mmap_content_set_checked_manifest_open_can_cancel_before_archive_probe() {
    let dir = temp_dir("gfm-content-manifest-set-open-cancel");
    let manifest_path = dir.join("content.gfmmanifest");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        &manifest_path,
        "gfm-content-manifest-v1\narchive\thot\tmissing.gfmcontent\n",
    )
    .unwrap();
    let mut checks = 0;

    let result = MmapContentSet::open_manifest_checked(&manifest_path, || {
        checks += 1;
        if checks >= 10 {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!dir.join("missing.gfmcontent").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn content_archive_manifest_rejects_duplicate_constructed_archive_paths() {
    let err = ContentArchiveManifest::new(vec![
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path: PathBuf::from("active.gfmcontent"),
        },
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Warm,
            path: PathBuf::from("active.gfmcontent"),
        },
    ])
    .unwrap_err();

    assert!(err
        .to_string()
        .contains("content manifest has duplicate archive path `active.gfmcontent`"));
}

#[test]
fn content_archive_manifest_rejects_duplicate_persisted_archive_paths_with_line_number() {
    let dir = temp_dir("gfm-content-manifest-duplicate-archive");
    let manifest_path = dir.join("content.gfmmanifest");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        &manifest_path,
        "gfm-content-manifest-v1\narchive\thot\tactive.gfmcontent\narchive\twarm\tactive.gfmcontent\n",
    )
    .unwrap();

    let err = ContentArchiveManifest::read(&manifest_path).unwrap_err();

    assert!(err
        .to_string()
        .contains(&format!("{} line 3", manifest_path.display())));
    assert!(err
        .to_string()
        .contains("duplicate content archive path `active.gfmcontent`"));
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
fn checked_content_manifest_promotion_cancels_before_journal_write() {
    let dir = temp_dir("gfm-content-manifest-promote-cancel");
    let manifest_path = dir.join("content.gfmmanifest");
    std::fs::create_dir_all(&dir).unwrap();
    write_content_postings(dir.join("warm-b.gfmcontent"), &[]).unwrap();

    let manifest = ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: PathBuf::from("hot-a.gfmcontent"),
    }])
    .unwrap();
    manifest.write(&manifest_path).unwrap();
    let mut checks = 0;

    let result = promote_content_archive_manifest_checked(
        &manifest_path,
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Warm,
            path: PathBuf::from("warm-b.gfmcontent"),
        },
        &[PathBuf::from("hot-a.gfmcontent")],
        || {
            checks += 1;
            if checks >= 10 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        },
    );

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert_eq!(
        ContentArchiveManifest::read(&manifest_path).unwrap(),
        manifest
    );
    assert!(!content_manifest_promotion_journal_path(&manifest_path).exists());
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
fn content_promotion_journal_rejects_duplicate_retired_paths() {
    let previous = ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: PathBuf::from("hot-a.gfmcontent"),
    }])
    .unwrap();

    let err = ContentManifestPromotionJournal::new(
        previous,
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Warm,
            path: PathBuf::from("warm-b.gfmcontent"),
        },
        vec![
            PathBuf::from("hot-a.gfmcontent"),
            PathBuf::from("hot-a.gfmcontent"),
        ],
    )
    .unwrap_err();

    assert!(err.to_string().contains(
        "content promotion journal has duplicate retired archive path `hot-a.gfmcontent`"
    ));
}

#[test]
fn content_promotion_journal_read_rejects_duplicate_previous_paths_with_line_number() {
    let dir = temp_dir("gfm-content-manifest-promotion-duplicate-previous");
    let manifest_path = dir.join("content.gfmmanifest");
    let journal_path = content_manifest_promotion_journal_path(&manifest_path);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        &journal_path,
        "gfm-content-promotion-journal-v1\nprevious\thot\thot-a.gfmcontent\nprevious\twarm\thot-a.gfmcontent\nnew\twarm\twarm-b.gfmcontent\nretire\thot-a.gfmcontent\n",
    )
    .unwrap();

    let err = ContentManifestPromotionJournal::read(&journal_path).unwrap_err();

    assert!(err
        .to_string()
        .contains(&format!("{} line 3", journal_path.display())));
    assert!(err
        .to_string()
        .contains("duplicate previous archive path `hot-a.gfmcontent`"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn content_promotion_journal_checked_read_honors_pre_cancelled_control_before_file_open() {
    let dir = temp_dir("gfm-content-manifest-promotion-journal-read-cancel");
    let manifest_path = dir.join("content.gfmmanifest");
    let journal_path = content_manifest_promotion_journal_path(&manifest_path);
    std::fs::create_dir_all(&dir).unwrap();

    let result =
        ContentManifestPromotionJournal::read_checked(&journal_path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!journal_path.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn content_promotion_journal_read_rejects_duplicate_retired_paths_with_line_number() {
    let dir = temp_dir("gfm-content-manifest-promotion-duplicate-retired");
    let manifest_path = dir.join("content.gfmmanifest");
    let journal_path = content_manifest_promotion_journal_path(&manifest_path);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        &journal_path,
        "gfm-content-promotion-journal-v1\nprevious\thot\thot-a.gfmcontent\nnew\twarm\twarm-b.gfmcontent\nretire\thot-a.gfmcontent\nretire\thot-a.gfmcontent\n",
    )
    .unwrap();

    let err = ContentManifestPromotionJournal::read(&journal_path).unwrap_err();

    assert!(err
        .to_string()
        .contains(&format!("{} line 5", journal_path.display())));
    assert!(err
        .to_string()
        .contains("duplicate retired archive path `hot-a.gfmcontent`"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checked_content_promotion_journal_read_honors_pre_cancelled_control_before_file_open() {
    let dir = temp_dir("gfm-content-promotion-journal-read-cancel");
    let manifest_path = dir.join("content.gfmmanifest");
    let journal_path = content_manifest_promotion_journal_path(&manifest_path);
    std::fs::create_dir_all(&dir).unwrap();

    let result =
        ContentManifestPromotionJournal::read_checked(&journal_path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!journal_path.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checked_content_manifest_promotion_recovery_plan_honors_pre_cancelled_control_before_journal_probe(
) {
    let dir = temp_dir("gfm-content-promotion-recovery-plan-cancel");
    let manifest_path = dir.join("content.gfmmanifest");
    let journal_path = content_manifest_promotion_journal_path(&manifest_path);
    std::fs::create_dir_all(&dir).unwrap();

    let result = plan_content_manifest_promotion_recovery_checked(&manifest_path, || {
        Err(GfmError::Cancelled)
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(!journal_path.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checked_content_manifest_promotion_recovery_plan_cancels_before_journal_probe() {
    let dir = temp_dir("gfm-content-manifest-promotion-plan-cancel");
    let manifest_path = dir.join("content.gfmmanifest");
    std::fs::create_dir_all(&dir).unwrap();

    let result = plan_content_manifest_promotion_recovery_checked(&manifest_path, || {
        Err(GfmError::Cancelled)
    });

    assert!(matches!(result, Err(GfmError::Cancelled)));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checked_content_manifest_promotion_recovery_cancels_before_mutation() {
    let dir = temp_dir("gfm-content-manifest-promotion-recover-cancel");
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
        previous.clone(),
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Warm,
            path: PathBuf::from("warm-b.gfmcontent"),
        },
        vec![PathBuf::from("hot-a.gfmcontent")],
    )
    .unwrap();
    let journal_path = content_manifest_promotion_journal_path(&manifest_path);
    journal.write(&journal_path).unwrap();

    let result =
        recover_content_manifest_promotion_checked(&manifest_path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert_eq!(
        ContentArchiveManifest::read(&manifest_path).unwrap(),
        previous
    );
    assert!(journal_path.exists());
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
fn content_manifest_promotion_recovery_surfaces_journal_probe_failures() {
    let dir = temp_dir("gfm-content-manifest-promotion-probe");
    std::fs::create_dir_all(&dir).unwrap();
    let manifest_path = dir.join("content-manifest-promotion-unavailable".repeat(64));

    let plan = plan_content_manifest_promotion_recovery(&manifest_path);

    assert_eq!(
        plan.action,
        ContentManifestPromotionRecoveryAction::CannotRecover
    );
    assert!(plan
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("content promotion journal existence unavailable")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checked_content_manifest_promotion_recovery_honors_pre_cancelled_control_before_mutation() {
    let dir = temp_dir("gfm-content-promotion-recovery-cancel");
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
        previous.clone(),
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Warm,
            path: PathBuf::from("warm-b.gfmcontent"),
        },
        vec![PathBuf::from("hot-a.gfmcontent")],
    )
    .unwrap();
    let journal_path = content_manifest_promotion_journal_path(&manifest_path);
    journal.write(&journal_path).unwrap();

    let result =
        recover_content_manifest_promotion_checked(&manifest_path, || Err(GfmError::Cancelled));

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert_eq!(
        ContentArchiveManifest::read(&manifest_path).unwrap(),
        previous
    );
    assert!(journal_path.exists());
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
fn checked_content_archive_cleanup_cancels_before_removing_inactive_candidate() {
    let dir = temp_dir("gfm-content-manifest-cleanup-cancel");
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
    let mut checks = 0;

    let result = cleanup_inactive_content_archives_checked(
        &manifest_path,
        &[PathBuf::from("inactive.gfmcontent")],
        || {
            checks += 1;
            if checks >= 10 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        },
    );

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(inactive.exists());
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

#[test]
fn checked_content_archive_cleanup_plan_cancels_before_retired_metadata_probe() {
    let dir = temp_dir("gfm-content-manifest-cleanup-plan-cancel");
    let manifest_path = dir.join("content.gfmmanifest");
    let active = dir.join("active.gfmcontent");
    let retired = dir.join("retired.gfmcontent");
    std::fs::create_dir_all(&dir).unwrap();
    write_content_postings(&active, &[]).unwrap();
    write_content_postings(&retired, &[]).unwrap();

    ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
        tier: ContentMergeTier::Hot,
        path: PathBuf::from("active.gfmcontent"),
    }])
    .unwrap()
    .write(&manifest_path)
    .unwrap();
    let mut checks = 0;

    let result = plan_inactive_content_archive_cleanup_checked(
        &manifest_path,
        &[PathBuf::from("retired.gfmcontent")],
        &ContentArchiveCleanupPolicy::default(),
        || {
            checks += 1;
            if checks >= 12 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        },
    );

    assert!(matches!(result, Err(GfmError::Cancelled)));
    assert!(active.exists());
    assert!(retired.exists());
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
