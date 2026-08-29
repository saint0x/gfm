use super::*;
use crate::copy::copy_file;
use crate::journal::{append_journal, now_nanos};
#[cfg(target_os = "macos")]
use crate::preserve::acl_copy_unsupported;
#[cfg(target_vendor = "apple")]
use crate::preserve::file_flag_preservation_unsupported;
use crate::preserve::{time_preservation_unsupported, xattr_copy_unsupported};
use crate::progress::ProgressTracker;
use crate::target::path_exists_or_symlink;
use crate::transfer::{
    clone_fallback_allowed, clone_file, copy_file_bytes, copy_file_bytes_tracked,
    metadata_has_sparse_holes,
};
use crate::trashmeta::{append_trash_metadata, append_trash_metadata_entry};
use crate::verify::verify_copy;
use crate::volume::{COPY_BUFFER_BYTES, SLOW_COPY_BUFFER_BYTES};
use gfm_types::GfmError;
use std::fs;
use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::PathBuf;

#[test]
fn copies_directories_and_records_journal() {
    let root = unique_temp_dir("gfm-ops-copy");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested").join("file.txt"), "hello").unwrap();

    let operator = Operator::new(OperationContext::new(&journal));
    let entry = operator
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(entry.status, OperationStatus::Completed);
    assert_eq!(
        fs::read_to_string(destination.join("nested").join("file.txt")).unwrap(),
        "hello"
    );
    let journal_entries = operator.journal().unwrap();
    assert_eq!(journal_entries.len(), 2);
    assert_eq!(journal_entries[0].status, OperationStatus::Started);
    assert_eq!(journal_entries[1].status, OperationStatus::Completed);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn plans_recursive_copy_totals_before_execution() {
    let root = unique_temp_dir("gfm-ops-plan-copy");
    let source = root.join("source");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("alpha.txt"), "alpha").unwrap();
    fs::write(source.join("nested").join("beta.txt"), "beta").unwrap();

    let progress = plan_operation(&Operation::Copy {
        from: source.clone(),
        to: root.join("destination"),
    })
    .unwrap();

    assert_eq!(progress.total_items, 4);
    assert_eq!(progress.total_bytes, 9);
    assert_eq!(progress.completed_items, 0);
    assert_eq!(progress.completed_bytes, 0);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_emits_planned_and_advanced_progress() {
    let root = unique_temp_dir("gfm-ops-progress-copy");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("first.txt"), "first").unwrap();
    fs::write(source.join("nested").join("second.txt"), "second").unwrap();
    let mut events = Vec::new();

    Operator::new(OperationContext::new(&journal))
        .execute_with_progress(
            Operation::Copy {
                from: source.clone(),
                to: destination,
            },
            |event| events.push(event),
        )
        .unwrap();

    assert_eq!(
        events.first().unwrap().phase,
        OperationProgressPhase::Planned
    );
    assert_eq!(events.first().unwrap().progress.total_items, 4);
    assert_eq!(events.first().unwrap().progress.total_bytes, 11);
    let last = events.last().unwrap();
    assert_eq!(last.phase, OperationProgressPhase::Advanced);
    assert_eq!(last.progress.completed_items, 4);
    assert_eq!(last.progress.completed_bytes, 11);
    assert_eq!(last.progress.completed_items, last.progress.total_items);
    assert_eq!(last.progress.completed_bytes, last.progress.total_bytes);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delete_completes_recursive_progress_after_success() {
    let root = unique_temp_dir("gfm-ops-progress-delete");
    let journal = root.join("journal.log");
    let target = root.join("target");
    fs::create_dir_all(target.join("nested")).unwrap();
    fs::write(target.join("nested").join("payload.txt"), "payload").unwrap();
    let mut events = Vec::new();

    Operator::new(OperationContext::new(&journal))
        .execute_with_progress(Operation::Delete { path: target }, |event| {
            events.push(event);
        })
        .unwrap();

    assert_eq!(
        events.first().unwrap().phase,
        OperationProgressPhase::Planned
    );
    assert_eq!(events.first().unwrap().progress.total_items, 3);
    assert_eq!(events.first().unwrap().progress.total_bytes, 7);
    let last = events.last().unwrap();
    assert_eq!(last.phase, OperationProgressPhase::Advanced);
    assert_eq!(last.progress.completed_items, 3);
    assert_eq!(last.progress.completed_bytes, 7);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_before_preflight_journals_cancelled_without_progress() {
    let root = unique_temp_dir("gfm-ops-cancel-preflight");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("payload.txt"), "payload").unwrap();
    let cancellation = OperationCancellation::default();
    cancellation.cancel();
    let mut events = Vec::new();

    let err = Operator::new(OperationContext::new(&journal).with_cancellation(cancellation))
        .execute_with_progress(
            Operation::Copy {
                from: source,
                to: destination,
            },
            |event| events.push(event),
        )
        .unwrap_err();

    assert!(matches!(err, GfmError::Cancelled));
    assert!(events.is_empty());
    let journal_entries = read_journal(&journal).unwrap();
    assert_eq!(journal_entries.len(), 2);
    assert_eq!(journal_entries[0].status, OperationStatus::Started);
    assert_eq!(journal_entries[1].status, OperationStatus::Cancelled);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recursive_copy_stops_after_cancellation_checkpoint() {
    let root = unique_temp_dir("gfm-ops-cancel-copy");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("first.txt"), "first").unwrap();
    fs::write(source.join("nested").join("second.txt"), "second").unwrap();
    let cancellation = OperationCancellation::default();
    let cancellation_callback = cancellation.clone();
    let mut events = Vec::new();

    let err = Operator::new(OperationContext::new(&journal).with_cancellation(cancellation))
        .execute_with_progress(
            Operation::Copy {
                from: source,
                to: destination.clone(),
            },
            |event| {
                if event.phase == OperationProgressPhase::Advanced
                    && event.progress.completed_items == 1
                {
                    cancellation_callback.cancel();
                }
                events.push(event);
            },
        )
        .unwrap_err();

    assert!(matches!(err, GfmError::Cancelled));
    assert!(!path_exists_or_symlink(&destination));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.phase == OperationProgressPhase::Advanced)
            .count(),
        1
    );
    let journal_entries = read_journal(&journal).unwrap();
    assert_eq!(
        journal_entries.last().unwrap().status,
        OperationStatus::Cancelled
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn paused_recursive_copy_journals_recoverable_pause() {
    let root = unique_temp_dir("gfm-ops-pause-copy");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested").join("second.txt"), "second").unwrap();
    fs::write(source.join("first.txt"), "first").unwrap();
    let pause = OperationPause::default();
    let pause_callback = pause.clone();
    let mut events = Vec::new();

    let err = Operator::new(OperationContext::new(&journal).with_pause(pause))
        .execute_with_progress(
            Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            },
            |event| {
                if event.phase == OperationProgressPhase::Advanced
                    && event.progress.completed_items == 1
                {
                    pause_callback.pause();
                }
                events.push(event);
            },
        )
        .unwrap_err();

    assert!(matches!(err, GfmError::Paused));
    assert!(destination.is_dir());
    assert!(!destination.join("first.txt").exists());
    let entries = read_journal(&journal).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].status, OperationStatus::Started);
    assert_eq!(entries[1].status, OperationStatus::Paused);
    assert!(events
        .iter()
        .any(|event| event.phase == OperationProgressPhase::Advanced
            && event.progress.completed_items == 1));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovery_resumes_paused_directory_copy_into_existing_destination() {
    let root = unique_temp_dir("gfm-ops-resume-paused-copy");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("first.txt"), "first").unwrap();
    fs::write(source.join("nested").join("second.txt"), "second").unwrap();
    fs::create_dir_all(&destination).unwrap();
    let operation = Operation::Copy {
        from: source.clone(),
        to: destination.clone(),
    };
    append_journal(&journal, &JournalEntry::started(46, operation.clone())).unwrap();
    append_journal(&journal, &JournalEntry::paused(46, operation)).unwrap();

    let report = Operator::new(OperationContext::new(&journal))
        .recover_interrupted()
        .unwrap();

    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(report.outcomes[0].id, 46);
    assert_eq!(report.outcomes[0].status, OperationStatus::Completed);
    assert_eq!(
        fs::read_to_string(destination.join("first.txt")).unwrap(),
        "first"
    );
    assert_eq!(
        fs::read_to_string(destination.join("nested").join("second.txt")).unwrap(),
        "second"
    );
    let entries = read_journal(&journal).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0].status, OperationStatus::Started);
    assert_eq!(entries[1].status, OperationStatus::Paused);
    assert_eq!(entries[2].status, OperationStatus::Started);
    assert_eq!(entries[3].status, OperationStatus::Completed);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_file_reports_method_and_preserves_contents() {
    let root = unique_temp_dir("gfm-ops-copy-method");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "clone-aware copy").unwrap();

    let method = copy_file(&source, &destination, VerificationPolicy::Bytes).unwrap();

    assert!(matches!(
        method,
        CopyMethod::ApfsClone | CopyMethod::ByteCopy
    ));
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "clone-aware copy"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn byte_copy_streams_large_file_with_bounded_buffer() {
    let root = unique_temp_dir("gfm-ops-byte-copy-stream");
    let source = root.join("source.bin");
    let destination = root.join("destination.bin");
    let mut bytes = Vec::with_capacity((COPY_BUFFER_BYTES * 2) + 17);
    for index in 0..((COPY_BUFFER_BYTES * 2) + 17) {
        bytes.push((index % 251) as u8);
    }
    fs::write(&source, &bytes).unwrap();

    let copied = copy_file_bytes(&source, &destination).unwrap();

    assert_eq!(copied, bytes.len() as u64);
    assert_eq!(fs::read(&destination).unwrap(), bytes);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn byte_copy_preserves_mixed_zero_and_nonzero_runs() {
    let root = unique_temp_dir("gfm-ops-byte-copy-zero-runs");
    let source = root.join("source.bin");
    let destination = root.join("destination.bin");
    let mut bytes = vec![0_u8; COPY_BUFFER_BYTES + 41];
    bytes[0] = 1;
    bytes[17] = 2;
    bytes[COPY_BUFFER_BYTES - 1] = 3;
    bytes[COPY_BUFFER_BYTES] = 4;
    bytes[COPY_BUFFER_BYTES + 40] = 5;
    fs::write(&source, &bytes).unwrap();

    let copied = copy_file_bytes(&source, &destination).unwrap();

    assert_eq!(copied, bytes.len() as u64);
    assert_eq!(fs::read(&destination).unwrap(), bytes);
    assert_eq!(
        fs::metadata(&destination).unwrap().len(),
        bytes.len() as u64
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn sparse_metadata_detects_zero_block_holes() {
    use std::os::unix::fs::MetadataExt;

    let root = unique_temp_dir("gfm-ops-sparse-metadata");
    let source = root.join("source.bin");
    let logical_len = COPY_BUFFER_BYTES as u64 * 4;
    {
        let file = File::create(&source).unwrap();
        file.set_len(logical_len).unwrap();
    }

    let metadata = fs::metadata(&source).unwrap();
    if metadata.blocks().saturating_mul(512) >= metadata.len() {
        fs::remove_dir_all(root).unwrap();
        return;
    }

    assert!(metadata_has_sparse_holes(&metadata));
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn byte_copy_preserves_sparse_holes_when_host_reports_blocks() {
    use std::os::unix::fs::MetadataExt;

    let root = unique_temp_dir("gfm-ops-byte-copy-sparse");
    let source = root.join("source.bin");
    let destination = root.join("destination.bin");
    let logical_len = (COPY_BUFFER_BYTES as u64 * 8) + 13;
    {
        let mut file = File::create(&source).unwrap();
        file.write_all(b"head").unwrap();
        file.seek(SeekFrom::Start(logical_len - 4)).unwrap();
        file.write_all(b"tail").unwrap();
    }
    let source_metadata = fs::metadata(&source).unwrap();
    if source_metadata.blocks() * 512 >= source_metadata.len() {
        fs::remove_dir_all(root).unwrap();
        return;
    }

    let copied = copy_file_bytes(&source, &destination).unwrap();

    let destination_metadata = fs::metadata(&destination).unwrap();
    assert_eq!(copied, logical_len);
    assert_eq!(destination_metadata.len(), logical_len);
    assert_eq!(fs::read(&destination).unwrap(), fs::read(&source).unwrap());
    assert!(
        destination_metadata.blocks() <= source_metadata.blocks() + 8,
        "expected sparse destination blocks <= source blocks plus tolerance, source={} destination={}",
        source_metadata.blocks(),
        destination_metadata.blocks()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn byte_copy_reports_chunk_progress() {
    let root = unique_temp_dir("gfm-ops-byte-copy-progress");
    let source = root.join("source.bin");
    let destination = root.join("destination.bin");
    let bytes = vec![7_u8; (COPY_BUFFER_BYTES * 2) + 19];
    fs::write(&source, &bytes).unwrap();
    let cancellation = OperationCancellation::default();
    let pause = OperationPause::default();
    let plan = OperationProgress {
        total_items: 1,
        total_bytes: bytes.len() as u64,
        completed_items: 0,
        completed_bytes: 0,
    };
    let mut events = Vec::new();
    let mut callback = |event| events.push(event);
    let mut tracker = ProgressTracker::new(plan, &cancellation, &pause, &mut callback);

    let copied = copy_file_bytes_tracked(
        &source,
        &destination,
        &OperationVolumeCopyPolicy::default(),
        &mut tracker,
    )
    .unwrap();
    tracker.finish_current_item().unwrap();

    assert_eq!(copied, bytes.len() as u64);
    assert!(events
        .iter()
        .any(|event| event.phase == OperationProgressPhase::Advanced
            && event.progress.completed_items == 0
            && event.progress.completed_bytes == COPY_BUFFER_BYTES as u64
            && event.throughput.is_some()));
    let last = events.last().unwrap();
    assert_eq!(last.progress.completed_items, 1);
    assert_eq!(last.progress.completed_bytes, bytes.len() as u64);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn throughput_snapshot_classifies_slow_and_constrained_transfers() {
    let slow = OperationThroughputSnapshot::classify(8 * 1024 * 1024, 1_000_000_000).unwrap();
    let constrained =
        OperationThroughputSnapshot::classify(32 * 1024 * 1024, 1_000_000_000).unwrap();
    let full_speed =
        OperationThroughputSnapshot::classify(256 * 1024 * 1024, 1_000_000_000).unwrap();

    assert_eq!(slow.class, OperationThroughputClass::Slow);
    assert_eq!(constrained.class, OperationThroughputClass::Constrained);
    assert_eq!(full_speed.class, OperationThroughputClass::FullSpeed);
    assert_eq!(OperationThroughputSnapshot::classify(0, 1_000), None);
}

#[test]
fn byte_copy_uses_slow_volume_checkpoint_chunks() {
    let root = unique_temp_dir("gfm-ops-byte-copy-slow-volume");
    let source_root = root.join("network-source");
    let destination_root = root.join("network-destination");
    let source = source_root.join("source.bin");
    let destination = destination_root.join("destination.bin");
    fs::create_dir_all(&source_root).unwrap();
    fs::create_dir_all(&destination_root).unwrap();
    let bytes = vec![5_u8; SLOW_COPY_BUFFER_BYTES + 11];
    fs::write(&source, &bytes).unwrap();
    let policy = OperationVolumeCopyPolicy::default()
        .with_root(&source_root, OperationVolumeClass::Network)
        .with_root(&destination_root, OperationVolumeClass::Network);
    let cancellation = OperationCancellation::default();
    let pause = OperationPause::default();
    let plan = OperationProgress {
        total_items: 1,
        total_bytes: bytes.len() as u64,
        completed_items: 0,
        completed_bytes: 0,
    };
    let mut events = Vec::new();
    let mut callback = |event| events.push(event);
    let mut tracker = ProgressTracker::new(plan, &cancellation, &pause, &mut callback);

    let copied = copy_file_bytes_tracked(&source, &destination, &policy, &mut tracker).unwrap();
    tracker.finish_current_item().unwrap();

    assert_eq!(copied, bytes.len() as u64);
    assert_eq!(fs::read(&destination).unwrap(), bytes);
    assert!(events
        .iter()
        .any(|event| event.phase == OperationProgressPhase::Advanced
            && event.progress.completed_items == 0
            && event.progress.completed_bytes == SLOW_COPY_BUFFER_BYTES as u64));
    assert!(!events
        .iter()
        .any(|event| event.progress.completed_bytes == COPY_BUFFER_BYTES as u64));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn byte_copy_cancellation_removes_partial_destination() {
    let root = unique_temp_dir("gfm-ops-byte-copy-cancel");
    let source = root.join("source.bin");
    let destination = root.join("destination.bin");
    let bytes = vec![9_u8; (COPY_BUFFER_BYTES * 2) + 5];
    fs::write(&source, &bytes).unwrap();
    let cancellation = OperationCancellation::default();
    let cancellation_callback = cancellation.clone();
    let pause = OperationPause::default();
    let plan = OperationProgress {
        total_items: 1,
        total_bytes: bytes.len() as u64,
        completed_items: 0,
        completed_bytes: 0,
    };
    let mut events = Vec::new();
    let mut callback = |event: OperationProgressEvent| {
        if event.phase == OperationProgressPhase::Advanced && event.progress.completed_bytes > 0 {
            cancellation_callback.cancel();
        }
        events.push(event);
    };
    let mut tracker = ProgressTracker::new(plan, &cancellation, &pause, &mut callback);

    let err = copy_file_bytes_tracked(
        &source,
        &destination,
        &OperationVolumeCopyPolicy::default(),
        &mut tracker,
    )
    .unwrap_err();

    assert!(matches!(err, GfmError::Cancelled));
    assert!(!destination.exists());
    assert!(events
        .iter()
        .any(|event| event.phase == OperationProgressPhase::Advanced
            && event.progress.completed_bytes == COPY_BUFFER_BYTES as u64));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn byte_copy_refuses_existing_destination() {
    let root = unique_temp_dir("gfm-ops-byte-copy-existing");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "fresh").unwrap();
    fs::write(&destination, "existing").unwrap();

    let err = copy_file_bytes(&source, &destination).unwrap_err();

    assert!(
        matches!(err, GfmError::Io { .. }),
        "expected io conflict, got {err:?}"
    );
    assert_eq!(fs::read_to_string(&destination).unwrap(), "existing");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_verification_rejects_size_mismatch() {
    let root = unique_temp_dir("gfm-ops-verify-size");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "complete").unwrap();
    fs::write(&destination, "short").unwrap();

    let err = verify_copy(&source, &destination, VerificationPolicy::Size).unwrap_err();

    assert!(err.to_string().contains("source size"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_verification_rejects_byte_mismatch() {
    let root = unique_temp_dir("gfm-ops-verify-bytes");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "same length").unwrap();
    fs::write(&destination, "same Length").unwrap();

    let err = verify_copy(&source, &destination, VerificationPolicy::Bytes).unwrap_err();

    assert!(err.to_string().contains("byte mismatch"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_context_defaults_to_size_verification_for_fast_local_copies() {
    let root = unique_temp_dir("gfm-ops-default-verify-size");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "default verification").unwrap();
    let context = OperationContext::new(&journal);

    assert_eq!(context.verification, VerificationPolicy::Size);
    Operator::new(context)
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "default verification"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_can_use_size_only_verification_policy() {
    let root = unique_temp_dir("gfm-ops-verify-size-policy");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "policy").unwrap();

    Operator::new(OperationContext::new(&journal).with_verification(VerificationPolicy::Size))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(fs::read_to_string(&destination).unwrap(), "policy");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_can_opt_into_byte_verification_policy() {
    let root = unique_temp_dir("gfm-ops-verify-bytes-policy");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "byte policy").unwrap();

    Operator::new(OperationContext::new(&journal).with_verification(VerificationPolicy::Bytes))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(fs::read_to_string(&destination).unwrap(), "byte policy");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn copy_file_uses_apfs_clone_when_host_supports_it() {
    let root = unique_temp_dir("gfm-ops-apfs-clone");
    let source = root.join("source.bin");
    let probe = root.join("probe.bin");
    let destination = root.join("destination.bin");
    fs::write(&source, b"copy-on-write candidate").unwrap();

    match clone_file(&source, &probe) {
        Ok(()) => {
            fs::remove_file(&probe).unwrap();
            let method = copy_file(&source, &destination, VerificationPolicy::Bytes).unwrap();
            assert_eq!(method, CopyMethod::ApfsClone);
            assert_eq!(fs::read(&destination).unwrap(), b"copy-on-write candidate");
        }
        Err(err) if clone_fallback_allowed(&err) => {
            let method = copy_file(&source, &destination, VerificationPolicy::Bytes).unwrap();
            assert_eq!(method, CopyMethod::ByteCopy);
        }
        Err(err) => panic!("unexpected clonefile failure: {err}"),
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_preserves_xattrs_when_host_supports_them() {
    let root = unique_temp_dir("gfm-ops-xattrs");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "finder metadata").unwrap();
    match xattr::set(&source, "user.gfm.test", b"tagged") {
        Ok(()) => {}
        Err(err) if xattr_copy_unsupported(&err) => {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(err) => panic!("unexpected xattr setup failure: {err}"),
    }

    Operator::new(OperationContext::new(&journal))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(
        xattr::get(&destination, "user.gfm.test")
            .unwrap()
            .as_deref(),
        Some(b"tagged".as_slice())
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_preserves_modified_time() {
    let root = unique_temp_dir("gfm-ops-times");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "dated").unwrap();
    let expected = filetime::FileTime::from_unix_time(1_700_000_000, 123_000_000);
    filetime::set_file_mtime(&source, expected).unwrap();

    Operator::new(OperationContext::new(&journal))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    let copied = fs::metadata(&destination).unwrap();
    assert_eq!(
        filetime::FileTime::from_last_modification_time(&copied),
        expected
    );

    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_vendor = "apple")]
#[test]
fn copy_preserves_birthtime_when_host_supports_it() {
    use std::os::darwin::fs::FileTimesExt;
    use std::time::{Duration, UNIX_EPOCH};

    let root = unique_temp_dir("gfm-ops-birthtime");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "created").unwrap();
    let created = UNIX_EPOCH + Duration::from_secs(1_600_000_123);
    let file = File::open(&source).unwrap();
    match file.set_times(fs::FileTimes::new().set_created(created)) {
        Ok(()) => {}
        Err(err) if time_preservation_unsupported(&err) => {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(err) => panic!("unexpected birthtime setup failure: {err}"),
    }

    Operator::new(OperationContext::new(&journal))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(
        fs::metadata(&destination).unwrap().created().unwrap(),
        created
    );

    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_vendor = "apple")]
#[test]
fn copy_preserves_bsd_file_flags_when_host_supports_them() {
    use nix::sys::stat::FileFlag;
    use std::os::darwin::fs::MetadataExt;

    let root = unique_temp_dir("gfm-ops-bsd-flags");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "flags").unwrap();
    let flags = FileFlag::UF_HIDDEN;
    match nix::unistd::chflags(&source, flags) {
        Ok(()) => {}
        Err(err) => {
            let err = io::Error::from_raw_os_error(err as i32);
            if file_flag_preservation_unsupported(&err) {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            panic!("unexpected file flag setup failure: {err}");
        }
    }

    Operator::new(OperationContext::new(&journal))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    let copied_flags = fs::metadata(&destination).unwrap().st_flags();
    assert_eq!(copied_flags & flags.bits(), flags.bits());

    nix::unistd::chflags(&source, FileFlag::empty()).unwrap();
    nix::unistd::chflags(&destination, FileFlag::empty()).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_vendor = "apple")]
#[test]
fn delete_refuses_locked_file_before_remove() {
    use nix::sys::stat::FileFlag;

    let root = unique_temp_dir("gfm-ops-delete-locked-file");
    let journal = root.join("journal.log");
    let path = root.join("locked.txt");
    fs::write(&path, "locked").unwrap();
    match nix::unistd::chflags(&path, FileFlag::UF_IMMUTABLE) {
        Ok(()) => {}
        Err(err) => {
            let err = io::Error::from_raw_os_error(err as i32);
            if file_flag_preservation_unsupported(&err) {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            panic!("unexpected locked-file setup failure: {err}");
        }
    }

    let err = Operator::new(OperationContext::new(&journal))
        .execute(Operation::Delete { path: path.clone() })
        .unwrap_err();

    assert!(matches!(err, GfmError::Conflict { .. }));
    assert!(err.to_string().contains("locked-item confirmation"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "locked");

    nix::unistd::chflags(&path, FileFlag::empty()).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_vendor = "apple")]
#[test]
fn copy_replace_refuses_locked_destination_before_staging() {
    use nix::sys::stat::FileFlag;

    let root = unique_temp_dir("gfm-ops-copy-replace-locked");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "new").unwrap();
    fs::write(&destination, "old").unwrap();
    match nix::unistd::chflags(&destination, FileFlag::UF_IMMUTABLE) {
        Ok(()) => {}
        Err(err) => {
            let err = io::Error::from_raw_os_error(err as i32);
            if file_flag_preservation_unsupported(&err) {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            panic!("unexpected locked-file setup failure: {err}");
        }
    }

    let err = Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap_err();

    assert!(matches!(err, GfmError::Conflict { .. }));
    assert!(err.to_string().contains("locked-item confirmation"));
    assert_eq!(fs::read_to_string(&source).unwrap(), "new");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old");

    nix::unistd::chflags(&destination, FileFlag::empty()).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn copy_preserves_access_control_lists_when_host_supports_them() {
    let root = unique_temp_dir("gfm-ops-acls");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "acl").unwrap();
    let Ok(user) = std::env::var("USER") else {
        fs::remove_dir_all(root).unwrap();
        return;
    };
    if user.is_empty() {
        fs::remove_dir_all(root).unwrap();
        return;
    }
    let entries = vec![exacl::AclEntry::allow_user(
        &user,
        exacl::Perm::READ | exacl::Perm::READATTR | exacl::Perm::READSECURITY,
        None,
    )];
    let source_paths = [&source];
    match exacl::setfacl(&source_paths, &entries, None::<exacl::AclOption>) {
        Ok(()) => {}
        Err(err) if acl_copy_unsupported(&err) => {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(err) => panic!("unexpected acl setup failure: {err}"),
    }

    Operator::new(OperationContext::new(&journal))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(
        exacl::getfacl(&destination, None::<exacl::AclOption>).unwrap(),
        exacl::getfacl(&source, None::<exacl::AclOption>).unwrap()
    );

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn copy_preserves_owner_and_group_when_host_allows_it() {
    use std::os::unix::fs::MetadataExt;

    let root = unique_temp_dir("gfm-ops-ownership");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "ownership").unwrap();

    Operator::new(OperationContext::new(&journal))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    let source_metadata = fs::symlink_metadata(&source).unwrap();
    let destination_metadata = fs::symlink_metadata(&destination).unwrap();
    assert_eq!(destination_metadata.uid(), source_metadata.uid());
    assert_eq!(destination_metadata.gid(), source_metadata.gid());

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn copy_preserves_symlink_instead_of_copying_target() {
    let root = unique_temp_dir("gfm-ops-symlink");
    let journal = root.join("journal.log");
    let target = root.join("target.txt");
    let source = root.join("source-link");
    let destination = root.join("destination-link");
    fs::write(&target, "target bytes").unwrap();
    std::os::unix::fs::symlink(&target, &source).unwrap();

    Operator::new(OperationContext::new(&journal))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    let destination_metadata = fs::symlink_metadata(&destination).unwrap();
    assert!(destination_metadata.file_type().is_symlink());
    assert_eq!(fs::read_link(&destination).unwrap(), target);

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn copy_replace_symlink_uses_staged_destination() {
    let root = unique_temp_dir("gfm-ops-copy-replace-symlink");
    let journal = root.join("journal.log");
    let old_target = root.join("old-target.txt");
    let new_target = root.join("new-target.txt");
    let source = root.join("source-link");
    let destination = root.join("destination-link");
    fs::write(&old_target, "old").unwrap();
    fs::write(&new_target, "new").unwrap();
    std::os::unix::fs::symlink(&new_target, &source).unwrap();
    std::os::unix::fs::symlink(&old_target, &destination).unwrap();

    Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(fs::read_link(&source).unwrap(), new_target);
    assert_eq!(fs::read_link(&destination).unwrap(), new_target);
    let leaked_stage = fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".gfm-replace-")
        });
    assert!(!leaked_stage);

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn cancelled_copy_replace_symlink_preserves_existing_destination() {
    let root = unique_temp_dir("gfm-ops-copy-replace-symlink-cancel");
    let journal = root.join("journal.log");
    let old_target = root.join("old-target.txt");
    let new_target = root.join("new-target.txt");
    let source = root.join("source-link");
    let destination = root.join("destination-link");
    fs::write(&old_target, "old").unwrap();
    fs::write(&new_target, "new").unwrap();
    std::os::unix::fs::symlink(&new_target, &source).unwrap();
    std::os::unix::fs::symlink(&old_target, &destination).unwrap();
    let cancellation = OperationCancellation::default();
    let operator = Operator::new(
        OperationContext::new(&journal)
            .with_conflict(ConflictPolicy::Replace)
            .with_cancellation(cancellation.clone()),
    );

    let err = operator
        .execute_with_progress(
            Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            },
            |event| {
                if event.phase == OperationProgressPhase::Planned {
                    cancellation.cancel();
                }
            },
        )
        .unwrap_err();

    assert!(matches!(err, GfmError::Cancelled));
    assert_eq!(fs::read_link(&source).unwrap(), new_target);
    assert_eq!(fs::read_link(&destination).unwrap(), old_target);

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn copy_preserves_symlink_timestamps_when_host_supports_them() {
    let root = unique_temp_dir("gfm-ops-symlink-times");
    let journal = root.join("journal.log");
    let target = root.join("target.txt");
    let source = root.join("source-link");
    let destination = root.join("destination-link");
    fs::write(&target, "target bytes").unwrap();
    std::os::unix::fs::symlink(&target, &source).unwrap();
    let atime = filetime::FileTime::from_unix_time(1_650_000_000, 111_000_000);
    let mtime = filetime::FileTime::from_unix_time(1_650_000_123, 222_000_000);
    match filetime::set_symlink_file_times(&source, atime, mtime) {
        Ok(()) => {}
        Err(err) if time_preservation_unsupported(&err) => {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(err) => panic!("unexpected symlink time setup failure: {err}"),
    }

    Operator::new(OperationContext::new(&journal))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(
        filetime::FileTime::from_last_modification_time(
            &fs::symlink_metadata(&destination).unwrap()
        ),
        mtime
    );

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn recursive_copy_preserves_hard_link_topology() {
    use std::os::unix::fs::MetadataExt;

    let root = unique_temp_dir("gfm-ops-hard-links");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    let original = source.join("original.txt");
    let alias = source.join("nested").join("alias.txt");
    fs::write(&original, "shared inode").unwrap();
    match fs::hard_link(&original, &alias) {
        Ok(()) => {}
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
            ) =>
        {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(err) => panic!("unexpected hard-link setup failure: {err}"),
    }

    Operator::new(OperationContext::new(&journal))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    let copied_original = destination.join("original.txt");
    let copied_alias = destination.join("nested").join("alias.txt");
    assert_eq!(fs::read_to_string(&copied_alias).unwrap(), "shared inode");
    let original_metadata = fs::metadata(&copied_original).unwrap();
    let alias_metadata = fs::metadata(&copied_alias).unwrap();
    assert_eq!(original_metadata.dev(), alias_metadata.dev());
    assert_eq!(original_metadata.ino(), alias_metadata.ino());
    assert!(original_metadata.nlink() >= 2);

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn recursive_copy_degrades_hard_links_when_destination_volume_disallows_them() {
    use std::os::unix::fs::MetadataExt;

    let root = unique_temp_dir("gfm-ops-hard-links-unsupported");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    let original = source.join("original.txt");
    let alias = source.join("nested").join("alias.txt");
    fs::write(&original, "shared inode").unwrap();
    match fs::hard_link(&original, &alias) {
        Ok(()) => {}
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
            ) =>
        {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(err) => panic!("unexpected hard-link setup failure: {err}"),
    }
    let policy = OperationVolumeCopyPolicy::default()
        .with_root(&destination, OperationVolumeClass::Local)
        .with_root_hard_link_support(&destination, false);

    let mut events = Vec::new();
    Operator::new(OperationContext::new(&journal).with_volume_copy_policy(policy))
        .execute_with_progress(
            Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            },
            |event| events.push(event),
        )
        .unwrap();

    assert!(events.iter().any(|event| {
        event.phase == OperationProgressPhase::MetadataDegraded
            && event
                .metadata_degradation
                .as_ref()
                .is_some_and(|degradation| {
                    degradation.kind == OperationMetadataDegradationKind::HardLinkTopology
                        && degradation.path == destination.join("nested").join("alias.txt")
                })
    }));

    let copied_original = destination.join("original.txt");
    let copied_alias = destination.join("nested").join("alias.txt");
    assert_eq!(fs::read_to_string(&copied_alias).unwrap(), "shared inode");
    let original_metadata = fs::metadata(&copied_original).unwrap();
    let alias_metadata = fs::metadata(&copied_alias).unwrap();
    assert_eq!(original_metadata.dev(), alias_metadata.dev());
    assert_ne!(original_metadata.ino(), alias_metadata.ino());

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn copy_directory_preserves_nested_symlink() {
    let root = unique_temp_dir("gfm-ops-directory-symlink");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    let target = root.join("outside-target.txt");
    fs::create_dir_all(&source).unwrap();
    fs::write(&target, "outside").unwrap();
    std::os::unix::fs::symlink(&target, source.join("link")).unwrap();

    Operator::new(OperationContext::new(&journal))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    let copied_link = destination.join("link");
    assert!(fs::symlink_metadata(&copied_link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(copied_link).unwrap(), target);

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn copy_directory_applies_readonly_permissions_after_children() {
    use std::os::unix::fs::PermissionsExt;

    let root = unique_temp_dir("gfm-ops-readonly-directory");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested").join("file.txt"), "nested").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();

    Operator::new(OperationContext::new(&journal))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("nested").join("file.txt")).unwrap(),
        "nested"
    );
    assert_eq!(
        fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
        0o555
    );

    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_replace_directory_uses_staged_destination() {
    let root = unique_temp_dir("gfm-ops-copy-replace-directory");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested").join("new.txt"), "new").unwrap();
    fs::create_dir_all(destination.join("nested")).unwrap();
    fs::write(destination.join("nested").join("old.txt"), "old").unwrap();

    Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("nested").join("new.txt")).unwrap(),
        "new"
    );
    assert!(!destination.join("nested").join("old.txt").exists());
    assert_eq!(
        fs::read_to_string(source.join("nested").join("new.txt")).unwrap(),
        "new"
    );
    let leaked_replace_sibling = fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| entry.file_name().to_string_lossy().contains(".gfm-replace"));
    assert!(!leaked_replace_sibling);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_copy_replace_directory_preserves_existing_destination() {
    let root = unique_temp_dir("gfm-ops-copy-replace-directory-cancel");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested").join("new.txt"), "new").unwrap();
    fs::create_dir_all(destination.join("nested")).unwrap();
    fs::write(destination.join("nested").join("old.txt"), "old").unwrap();
    let cancellation = OperationCancellation::default();
    let cancellation_callback = cancellation.clone();

    let err = Operator::new(
        OperationContext::new(&journal)
            .with_conflict(ConflictPolicy::Replace)
            .with_cancellation(cancellation),
    )
    .execute_with_progress(
        Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        },
        |event| {
            if event.phase == OperationProgressPhase::Advanced
                && event.progress.completed_items == 1
            {
                cancellation_callback.cancel();
            }
        },
    )
    .unwrap_err();

    assert!(matches!(err, GfmError::Cancelled));
    assert_eq!(
        fs::read_to_string(destination.join("nested").join("old.txt")).unwrap(),
        "old"
    );
    assert!(!destination.join("nested").join("new.txt").exists());
    let leaked_replace_sibling = fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| entry.file_name().to_string_lossy().contains(".gfm-replace"));
    assert!(!leaked_replace_sibling);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fails_on_destination_conflict_without_mutating_source() {
    let root = unique_temp_dir("gfm-ops-conflict");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "source").unwrap();
    fs::write(&destination, "destination").unwrap();

    let operator = Operator::new(OperationContext::new(&journal));
    let err = operator
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap_err();

    assert!(matches!(err, GfmError::Conflict { .. }));
    assert_eq!(fs::read_to_string(&source).unwrap(), "source");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "destination");
    let journal_entries = operator.journal().unwrap();
    assert_eq!(journal_entries.len(), 2);
    assert_eq!(journal_entries[1].status, OperationStatus::Failed);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_replace_regular_file_uses_staged_destination() {
    let root = unique_temp_dir("gfm-ops-copy-replace-staged");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "new destination bytes").unwrap();
    fs::write(&destination, "old destination bytes").unwrap();

    Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "new destination bytes"
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "new destination bytes"
    );
    let leaked_stage = fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".gfm-replace-")
        });
    assert!(!leaked_stage);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_copy_replace_regular_file_preserves_existing_destination() {
    let root = unique_temp_dir("gfm-ops-copy-replace-cancel");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "new destination bytes").unwrap();
    fs::write(&destination, "old destination bytes").unwrap();
    let cancellation = OperationCancellation::default();
    let operator = Operator::new(
        OperationContext::new(&journal)
            .with_conflict(ConflictPolicy::Replace)
            .with_cancellation(cancellation.clone()),
    );

    let err = operator
        .execute_with_progress(
            Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            },
            |event| {
                if event.phase == OperationProgressPhase::Planned {
                    cancellation.cancel();
                }
            },
        )
        .unwrap_err();

    assert!(matches!(err, GfmError::Cancelled));
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "old destination bytes"
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "new destination bytes"
    );
    let leaked_stage = fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".gfm-replace-")
        });
    assert!(!leaked_stage);

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn copy_replace_same_inode_is_noop() {
    use std::os::unix::fs::MetadataExt;

    let root = unique_temp_dir("gfm-ops-copy-replace-same-inode");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "shared inode").unwrap();
    fs::hard_link(&source, &destination).unwrap();
    let before_source = fs::metadata(&source).unwrap();
    let before_destination = fs::metadata(&destination).unwrap();

    Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    let after_source = fs::metadata(&source).unwrap();
    let after_destination = fs::metadata(&destination).unwrap();
    assert_eq!(fs::read_to_string(&destination).unwrap(), "shared inode");
    assert_eq!(before_source.ino(), before_destination.ino());
    assert_eq!(after_source.ino(), after_destination.ino());
    assert_eq!(before_source.ino(), after_source.ino());
    assert!(after_source.nlink() >= 2);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_skip_conflict_journals_skipped_without_mutation() {
    let root = unique_temp_dir("gfm-ops-skip-copy");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "source").unwrap();
    fs::write(&destination, "destination").unwrap();
    let mut events = Vec::new();

    let entry = Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Skip))
        .execute_with_progress(
            Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            },
            |event| events.push(event),
        )
        .unwrap();

    assert_eq!(entry.status, OperationStatus::Skipped);
    assert!(events.is_empty());
    assert_eq!(fs::read_to_string(&source).unwrap(), "source");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "destination");
    let journal_entries = read_journal(&journal).unwrap();
    assert_eq!(journal_entries.len(), 2);
    assert_eq!(journal_entries[0].status, OperationStatus::Started);
    assert_eq!(journal_entries[1].status, OperationStatus::Skipped);
    assert_eq!(
        journal_entries[1].message.as_deref(),
        Some("operation skipped by conflict policy")
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovery_ignores_skipped_operations_as_terminal() {
    let root = unique_temp_dir("gfm-ops-recover-skipped");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "source").unwrap();
    fs::write(&destination, "destination").unwrap();
    let operation = Operation::Copy {
        from: source.clone(),
        to: destination.clone(),
    };
    append_journal(&journal, &JournalEntry::started(48, operation.clone())).unwrap();
    append_journal(&journal, &JournalEntry::skipped(48, operation)).unwrap();

    let report = Operator::new(OperationContext::new(&journal))
        .recover_with_policy(OperationRecoveryPolicy {
            retry_failed: true,
            max_attempts: 2,
        })
        .unwrap();

    assert!(report.outcomes.is_empty());
    assert_eq!(fs::read_to_string(&destination).unwrap(), "destination");
    assert_eq!(read_journal(&journal).unwrap().len(), 2);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_conflict_plan_applies_default_policy_to_all_targets() {
    let root = unique_temp_dir("gfm-ops-batch-apply-all");
    let journal = root.join("journal.log");
    let first_source = root.join("first-source.txt");
    let first_destination = root.join("first-destination.txt");
    let second_source = root.join("second-source.txt");
    let second_destination = root.join("second-destination.txt");
    fs::write(&first_source, "new first").unwrap();
    fs::write(&first_destination, "old first").unwrap();
    fs::write(&second_source, "new second").unwrap();
    fs::write(&second_destination, "old second").unwrap();

    let report = Operator::new(OperationContext::new(&journal))
        .execute_batch_with_conflicts(
            vec![
                Operation::Copy {
                    from: first_source.clone(),
                    to: first_destination.clone(),
                },
                Operation::Copy {
                    from: second_source.clone(),
                    to: second_destination.clone(),
                },
            ],
            OperationConflictPlan::new(ConflictPolicy::Skip),
        )
        .unwrap();

    assert_eq!(report.outcomes.len(), 2);
    assert!(report
        .outcomes
        .iter()
        .all(|outcome| outcome.conflict == ConflictPolicy::Skip
            && outcome.status == OperationStatus::Skipped));
    assert_eq!(fs::read_to_string(&first_destination).unwrap(), "old first");
    assert_eq!(
        fs::read_to_string(&second_destination).unwrap(),
        "old second"
    );
    let entries = read_journal(&journal).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.status == OperationStatus::Skipped)
            .count(),
        2
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_conflict_plan_uses_per_target_override() {
    let root = unique_temp_dir("gfm-ops-batch-per-target");
    let journal = root.join("journal.log");
    let replace_source = root.join("replace-source.txt");
    let replace_destination = root.join("replace-destination.txt");
    let skip_source = root.join("skip-source.txt");
    let skip_destination = root.join("skip-destination.txt");
    fs::write(&replace_source, "new replace").unwrap();
    fs::write(&replace_destination, "old replace").unwrap();
    fs::write(&skip_source, "new skip").unwrap();
    fs::write(&skip_destination, "old skip").unwrap();

    let report = Operator::new(OperationContext::new(&journal))
        .execute_batch_with_conflicts(
            vec![
                Operation::Copy {
                    from: replace_source.clone(),
                    to: replace_destination.clone(),
                },
                Operation::Copy {
                    from: skip_source.clone(),
                    to: skip_destination.clone(),
                },
            ],
            OperationConflictPlan::new(ConflictPolicy::Skip)
                .with_target(&replace_destination, ConflictPolicy::Replace),
        )
        .unwrap();

    assert_eq!(report.outcomes.len(), 2);
    assert_eq!(report.outcomes[0].conflict, ConflictPolicy::Replace);
    assert_eq!(report.outcomes[0].status, OperationStatus::Completed);
    assert_eq!(report.outcomes[1].conflict, ConflictPolicy::Skip);
    assert_eq!(report.outcomes[1].status, OperationStatus::Skipped);
    assert_eq!(
        fs::read_to_string(&replace_destination).unwrap(),
        "new replace"
    );
    assert_eq!(fs::read_to_string(&skip_destination).unwrap(), "old skip");
    let entries = read_journal(&journal).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[1].status, OperationStatus::Completed);
    assert_eq!(entries[3].status, OperationStatus::Skipped);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn merge_conflict_treats_finder_packages_as_atomic_items() {
    let root = unique_temp_dir("gfm-ops-package-merge");
    let journal = root.join("journal.log");
    let source = root.join("Demo.app");
    let destination = root.join("Demo Copy.app");
    fs::create_dir_all(source.join("Contents")).unwrap();
    fs::create_dir_all(destination.join("Contents")).unwrap();
    fs::write(source.join("Contents").join("new.txt"), "source").unwrap();
    fs::write(
        destination.join("Contents").join("existing.txt"),
        "destination",
    )
    .unwrap();

    let err = Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Merge))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap_err();

    assert!(matches!(err, GfmError::Conflict { .. }));
    assert!(!destination.join("Contents").join("new.txt").exists());
    assert_eq!(
        fs::read_to_string(destination.join("Contents").join("existing.txt")).unwrap(),
        "destination"
    );
    let entries = read_journal(&journal).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].status, OperationStatus::Started);
    assert_eq!(entries[1].status, OperationStatus::Failed);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_fresh_directory_copy_removes_incomplete_destination() {
    let root = unique_temp_dir("gfm-ops-directory-cancel");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested").join("file.txt"), "nested").unwrap();
    let cancellation = OperationCancellation::default();
    let cancellation_callback = cancellation.clone();

    let err = Operator::new(OperationContext::new(&journal).with_cancellation(cancellation))
        .execute_with_progress(
            Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            },
            |event| {
                if event.phase == OperationProgressPhase::Advanced
                    && event.progress.completed_items == 1
                {
                    cancellation_callback.cancel();
                }
            },
        )
        .unwrap_err();

    assert!(matches!(err, GfmError::Cancelled));
    assert!(!path_exists_or_symlink(&destination));
    let entries = read_journal(&journal).unwrap();
    assert_eq!(entries[1].status, OperationStatus::Cancelled);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_fresh_package_copy_removes_incomplete_bundle() {
    let root = unique_temp_dir("gfm-ops-package-cancel");
    let journal = root.join("journal.log");
    let source = root.join("Demo.app");
    let destination = root.join("Demo Copy.app");
    fs::create_dir_all(source.join("Contents").join("Resources")).unwrap();
    fs::write(source.join("Contents").join("Info.plist"), "plist").unwrap();
    fs::write(
        source.join("Contents").join("Resources").join("asset.txt"),
        "asset",
    )
    .unwrap();
    let cancellation = OperationCancellation::default();
    let cancellation_callback = cancellation.clone();

    let err = Operator::new(OperationContext::new(&journal).with_cancellation(cancellation))
        .execute_with_progress(
            Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            },
            |event| {
                if event.phase == OperationProgressPhase::Advanced
                    && event.progress.completed_items == 1
                {
                    cancellation_callback.cancel();
                }
            },
        )
        .unwrap_err();

    assert!(matches!(err, GfmError::Cancelled));
    assert!(!path_exists_or_symlink(&destination));
    let entries = read_journal(&journal).unwrap();
    assert_eq!(entries[1].status, OperationStatus::Cancelled);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_keep_both_allocates_finder_style_destination() {
    let root = unique_temp_dir("gfm-ops-keep-both-copy");
    let journal = root.join("journal.log");
    let source = root.join("report.md");
    let destination = root.join("destination.md");
    let copy_destination = root.join("destination copy.md");
    fs::write(&source, "source").unwrap();
    fs::write(&destination, "destination").unwrap();

    let entry =
        Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::KeepBoth))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

    assert_eq!(fs::read_to_string(&source).unwrap(), "source");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "destination");
    assert_eq!(fs::read_to_string(&copy_destination).unwrap(), "source");
    assert_eq!(
        entry.operation,
        Operation::Copy {
            from: source,
            to: copy_destination
        }
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn move_keep_both_allocates_next_available_destination() {
    let root = unique_temp_dir("gfm-ops-keep-both-move");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    let first_copy = root.join("destination copy.txt");
    let second_copy = root.join("destination copy 2.txt");
    fs::write(&source, "new").unwrap();
    fs::write(&destination, "old").unwrap();
    fs::write(&first_copy, "older").unwrap();

    let entry =
        Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::KeepBoth))
            .execute(Operation::Move {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

    assert!(!source.exists());
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old");
    assert_eq!(fs::read_to_string(&first_copy).unwrap(), "older");
    assert_eq!(fs::read_to_string(&second_copy).unwrap(), "new");
    assert_eq!(
        entry.operation,
        Operation::Move {
            from: source,
            to: second_copy
        }
    );

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn move_uses_copy_delete_when_policy_reports_distinct_volumes() {
    use std::os::unix::fs::MetadataExt;

    let root = unique_temp_dir("gfm-ops-move-distinct-volume-policy");
    let journal = root.join("journal.log");
    let source_root = root.join("SourceVolume");
    let destination_root = root.join("DestinationVolume");
    fs::create_dir_all(&source_root).unwrap();
    fs::create_dir_all(&destination_root).unwrap();
    let source = source_root.join("source.txt");
    let destination = destination_root.join("destination.txt");
    fs::write(&source, "move without doomed rename").unwrap();
    let source_metadata = fs::metadata(&source).unwrap();
    let policy = OperationVolumeCopyPolicy::default()
        .with_root_volume_identity(&source_root, "diskarbitration:uuid:SOURCE")
        .with_root_volume_identity(&destination_root, "diskarbitration:uuid:DESTINATION");

    Operator::new(OperationContext::new(&journal).with_volume_copy_policy(policy))
        .execute(Operation::Move {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    let destination_metadata = fs::metadata(&destination).unwrap();
    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "move without doomed rename"
    );
    assert_ne!(source_metadata.ino(), destination_metadata.ino());

    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_vendor = "apple")]
#[test]
fn move_copy_delete_refuses_locked_descendant_before_copying() {
    use nix::sys::stat::FileFlag;

    let root = unique_temp_dir("gfm-ops-move-locked-descendant");
    let journal = root.join("journal.log");
    let source_root = root.join("SourceVolume");
    let destination_root = root.join("DestinationVolume");
    let source = source_root.join("source");
    let locked = source.join("nested").join("locked.txt");
    let destination = destination_root.join("source");
    fs::create_dir_all(locked.parent().unwrap()).unwrap();
    fs::create_dir_all(&destination_root).unwrap();
    fs::write(&locked, "locked").unwrap();
    match nix::unistd::chflags(&locked, FileFlag::UF_IMMUTABLE) {
        Ok(()) => {}
        Err(err) => {
            let err = io::Error::from_raw_os_error(err as i32);
            if file_flag_preservation_unsupported(&err) {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            panic!("unexpected locked-file setup failure: {err}");
        }
    }
    let policy = OperationVolumeCopyPolicy::default()
        .with_root_volume_identity(&source_root, "diskarbitration:uuid:SOURCE")
        .with_root_volume_identity(&destination_root, "diskarbitration:uuid:DESTINATION");

    let err = Operator::new(OperationContext::new(&journal).with_volume_copy_policy(policy))
        .execute(Operation::Move {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap_err();

    assert!(matches!(err, GfmError::Conflict { .. }));
    assert!(err.to_string().contains("locked-item confirmation"));
    assert!(locked.exists());
    assert!(!destination.exists());

    nix::unistd::chflags(&locked, FileFlag::empty()).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_vendor = "apple")]
#[test]
fn move_rename_refuses_locked_descendant_before_renaming() {
    use nix::sys::stat::FileFlag;

    let root = unique_temp_dir("gfm-ops-move-rename-locked-descendant");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let locked = source.join("nested").join("locked.txt");
    let destination = root.join("destination");
    fs::create_dir_all(locked.parent().unwrap()).unwrap();
    fs::write(&locked, "locked").unwrap();
    match nix::unistd::chflags(&locked, FileFlag::UF_IMMUTABLE) {
        Ok(()) => {}
        Err(err) => {
            let err = io::Error::from_raw_os_error(err as i32);
            if file_flag_preservation_unsupported(&err) {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            panic!("unexpected locked-file setup failure: {err}");
        }
    }

    let err = Operator::new(OperationContext::new(&journal))
        .execute(Operation::Move {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap_err();

    assert!(matches!(err, GfmError::Conflict { .. }));
    assert!(err.to_string().contains("locked-item confirmation"));
    assert!(locked.exists());
    assert!(!destination.exists());

    nix::unistd::chflags(&locked, FileFlag::empty()).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_merge_combines_directories_without_overwriting_existing_files() {
    let root = unique_temp_dir("gfm-ops-merge-copy");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir_all(destination.join("nested")).unwrap();
    fs::write(source.join("nested").join("new.txt"), "new").unwrap();
    fs::write(destination.join("nested").join("old.txt"), "old").unwrap();

    let entry = Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Merge))
        .execute(Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert_eq!(entry.status, OperationStatus::Completed);
    assert_eq!(
        fs::read_to_string(destination.join("nested").join("new.txt")).unwrap(),
        "new"
    );
    assert_eq!(
        fs::read_to_string(destination.join("nested").join("old.txt")).unwrap(),
        "old"
    );
    assert_eq!(
        fs::read_to_string(source.join("nested").join("new.txt")).unwrap(),
        "new"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_merge_rejects_existing_file_conflict_without_overwrite() {
    let root = unique_temp_dir("gfm-ops-merge-conflict");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    fs::write(source.join("same.txt"), "source").unwrap();
    fs::write(destination.join("same.txt"), "destination").unwrap();

    let err = Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Merge))
        .execute(Operation::Copy {
            from: source,
            to: destination.clone(),
        })
        .unwrap_err();

    assert!(matches!(err, GfmError::Conflict { .. }));
    assert_eq!(
        fs::read_to_string(destination.join("same.txt")).unwrap(),
        "destination"
    );
    let entries = read_journal(&journal).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].status, OperationStatus::Started);
    assert_eq!(entries[1].status, OperationStatus::Failed);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn move_merge_combines_directories_and_removes_source_after_success() {
    let root = unique_temp_dir("gfm-ops-merge-move");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir_all(destination.join("nested")).unwrap();
    fs::write(source.join("nested").join("new.txt"), "new").unwrap();
    fs::write(destination.join("nested").join("old.txt"), "old").unwrap();

    Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Merge))
        .execute(Operation::Move {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(destination.join("nested").join("new.txt")).unwrap(),
        "new"
    );
    assert_eq!(
        fs::read_to_string(destination.join("nested").join("old.txt")).unwrap(),
        "old"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn trash_metadata_round_trips_original_restore_destination() {
    let root = unique_temp_dir("gfm-ops-trash-metadata");
    let metadata = root.join("trash.tsv");
    let original = root.join("Documents").join("report.md");
    fs::create_dir_all(original.parent().unwrap()).unwrap();

    append_trash_metadata(&metadata, &original).unwrap();

    let entries = read_trash_metadata(&metadata).unwrap();
    let entry = entries.get("report.md").unwrap();
    assert_eq!(entry.original_path, original);
    assert!(entry.can_restore);
    assert!(entry.can_delete_permanently);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restore_moves_trash_entry_to_metadata_destination_and_removes_metadata() {
    let root = unique_temp_dir("gfm-ops-restore");
    let journal = root.join("journal.log");
    let metadata = root.join("trash.tsv");
    let trash_dir = root.join("Trash");
    let original_dir = root.join("Documents");
    let trashed = trash_dir.join("report.md");
    let original = original_dir.join("report.md");
    fs::create_dir_all(&trash_dir).unwrap();
    fs::create_dir_all(&original_dir).unwrap();
    fs::write(&trashed, "restore me").unwrap();
    append_trash_metadata_entry(
        &metadata,
        &TrashRestoreMetadata {
            name: "report.md".to_string(),
            original_path: original.clone(),
            deleted_at_nanos: 7,
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        },
    )
    .unwrap();

    let entry = Operator::new(
        OperationContext::new(&journal)
            .with_trash_metadata_path(&metadata)
            .with_conflict(ConflictPolicy::Fail),
    )
    .execute(Operation::Restore {
        from: trashed.clone(),
        to: original.clone(),
    })
    .unwrap();

    assert_eq!(entry.status, OperationStatus::Completed);
    assert!(!trashed.exists());
    assert_eq!(fs::read_to_string(&original).unwrap(), "restore me");
    assert!(read_trash_metadata(&metadata).unwrap().is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restore_conflict_preserves_trash_entry_and_existing_destination() {
    let root = unique_temp_dir("gfm-ops-restore-conflict");
    let journal = root.join("journal.log");
    let metadata = root.join("trash.tsv");
    let trash_dir = root.join("Trash");
    let original_dir = root.join("Documents");
    let trashed = trash_dir.join("report.md");
    let original = original_dir.join("report.md");
    fs::create_dir_all(&trash_dir).unwrap();
    fs::create_dir_all(&original_dir).unwrap();
    fs::write(&trashed, "trashed").unwrap();
    fs::write(&original, "existing").unwrap();
    append_trash_metadata(&metadata, &original).unwrap();

    let err = Operator::new(OperationContext::new(&journal).with_trash_metadata_path(&metadata))
        .execute(Operation::Restore {
            from: trashed.clone(),
            to: original.clone(),
        })
        .unwrap_err();

    assert!(matches!(err, GfmError::Conflict { .. }));
    assert_eq!(fs::read_to_string(&trashed).unwrap(), "trashed");
    assert_eq!(fs::read_to_string(&original).unwrap(), "existing");
    assert!(read_trash_metadata(&metadata)
        .unwrap()
        .contains_key("report.md"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn permanent_delete_removes_trash_metadata_after_success() {
    let root = unique_temp_dir("gfm-ops-permanent-delete");
    let journal = root.join("journal.log");
    let metadata = root.join("trash.tsv");
    let trash_dir = root.join("Trash");
    let trashed = trash_dir.join("report.md");
    let original = root.join("Documents").join("report.md");
    fs::create_dir_all(&trash_dir).unwrap();
    fs::write(&trashed, "delete forever").unwrap();
    append_trash_metadata_entry(
        &metadata,
        &TrashRestoreMetadata {
            name: "report.md".to_string(),
            original_path: original,
            deleted_at_nanos: 9,
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        },
    )
    .unwrap();

    let entry = Operator::new(OperationContext::new(&journal).with_trash_metadata_path(&metadata))
        .execute(Operation::Delete {
            path: trashed.clone(),
        })
        .unwrap();

    assert_eq!(entry.status, OperationStatus::Completed);
    assert!(!trashed.exists());
    assert!(read_trash_metadata(&metadata).unwrap().is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_permanent_delete_preserves_trash_metadata() {
    let root = unique_temp_dir("gfm-ops-permanent-delete-fail");
    let journal = root.join("journal.log");
    let metadata = root.join("trash.tsv");
    let trashed = root.join("Trash").join("missing.md");
    let original = root.join("Documents").join("missing.md");
    append_trash_metadata_entry(
        &metadata,
        &TrashRestoreMetadata {
            name: "missing.md".to_string(),
            original_path: original,
            deleted_at_nanos: 10,
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        },
    )
    .unwrap();

    let err = Operator::new(OperationContext::new(&journal).with_trash_metadata_path(&metadata))
        .execute(Operation::Delete {
            path: trashed.clone(),
        })
        .unwrap_err();

    assert!(matches!(err, GfmError::Io { .. }));
    assert!(read_trash_metadata(&metadata)
        .unwrap()
        .contains_key("missing.md"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_trash_deletes_children_and_removes_metadata_after_success() {
    let root = unique_temp_dir("gfm-ops-empty-trash");
    let journal = root.join("journal.log");
    let metadata = root.join("trash.tsv");
    let trash_dir = root.join("Trash");
    let trashed_file = trash_dir.join("report.md");
    let trashed_dir = trash_dir.join("Old Folder");
    fs::create_dir_all(trashed_dir.join("nested")).unwrap();
    fs::write(&trashed_file, "delete file").unwrap();
    fs::write(trashed_dir.join("nested").join("note.txt"), "delete folder").unwrap();
    append_trash_metadata_entry(
        &metadata,
        &TrashRestoreMetadata {
            name: "report.md".to_string(),
            original_path: root.join("Documents").join("report.md"),
            deleted_at_nanos: 11,
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        },
    )
    .unwrap();
    append_trash_metadata_entry(
        &metadata,
        &TrashRestoreMetadata {
            name: "Old Folder".to_string(),
            original_path: root.join("Documents").join("Old Folder"),
            deleted_at_nanos: 12,
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        },
    )
    .unwrap();

    let mut progress = Vec::new();
    let entry = Operator::new(OperationContext::new(&journal).with_trash_metadata_path(&metadata))
        .execute_with_progress(
            Operation::EmptyTrash {
                path: trash_dir.clone(),
            },
            |event| progress.push(event),
        )
        .unwrap();

    assert_eq!(entry.status, OperationStatus::Completed);
    assert!(trash_dir.exists());
    assert!(fs::read_dir(&trash_dir).unwrap().next().is_none());
    assert!(read_trash_metadata(&metadata).unwrap().is_empty());
    assert_eq!(
        read_journal(&journal).unwrap().last().unwrap().operation,
        Operation::EmptyTrash { path: trash_dir }
    );
    assert_eq!(
        progress.last().unwrap().progress.completed_items,
        progress.last().unwrap().progress.total_items
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_trash_requires_directory_and_preserves_metadata() {
    let root = unique_temp_dir("gfm-ops-empty-trash-file");
    let journal = root.join("journal.log");
    let metadata = root.join("trash.tsv");
    let not_directory = root.join("Trash");
    fs::write(&not_directory, "not a directory").unwrap();
    append_trash_metadata_entry(
        &metadata,
        &TrashRestoreMetadata {
            name: "Trash".to_string(),
            original_path: root.join("Documents").join("Trash"),
            deleted_at_nanos: 13,
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        },
    )
    .unwrap();

    let err = Operator::new(OperationContext::new(&journal).with_trash_metadata_path(&metadata))
        .execute(Operation::EmptyTrash {
            path: not_directory.clone(),
        })
        .unwrap_err();

    assert!(matches!(err, GfmError::Format(message) if message.contains("requires a directory")));
    assert!(not_directory.exists());
    assert!(read_trash_metadata(&metadata)
        .unwrap()
        .contains_key("Trash"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_trash_reconciles_stale_metadata_for_missing_children() {
    let root = unique_temp_dir("gfm-ops-empty-trash-stale-metadata");
    let journal = root.join("journal.log");
    let metadata = root.join("trash.tsv");
    let trash_dir = root.join("Trash");
    fs::create_dir_all(&trash_dir).unwrap();
    append_trash_metadata_entry(
        &metadata,
        &TrashRestoreMetadata {
            name: "already-deleted.md".to_string(),
            original_path: root.join("Documents").join("already-deleted.md"),
            deleted_at_nanos: 14,
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        },
    )
    .unwrap();

    let entry = Operator::new(OperationContext::new(&journal).with_trash_metadata_path(&metadata))
        .execute(Operation::EmptyTrash {
            path: trash_dir.clone(),
        })
        .unwrap();

    assert_eq!(entry.status, OperationStatus::Completed);
    assert!(trash_dir.exists());
    assert!(fs::read_dir(&trash_dir).unwrap().next().is_none());
    assert!(read_trash_metadata(&metadata).unwrap().is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn journals_failed_preflight_for_missing_source() {
    let root = unique_temp_dir("gfm-ops-missing-source");
    let journal = root.join("journal.log");
    let source = root.join("missing.txt");
    let destination = root.join("destination.txt");

    let operator = Operator::new(OperationContext::new(&journal));
    let err = operator
        .execute(Operation::Copy {
            from: source,
            to: destination,
        })
        .unwrap_err();

    assert!(matches!(err, GfmError::Io { .. }));
    let journal_entries = operator.journal().unwrap();
    assert_eq!(journal_entries.len(), 2);
    assert_eq!(journal_entries[0].status, OperationStatus::Started);
    assert_eq!(journal_entries[1].status, OperationStatus::Failed);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovers_interrupted_copy_with_original_operation_id() {
    let root = unique_temp_dir("gfm-ops-recover-copy");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "recover me").unwrap();
    append_journal(
        &journal,
        &JournalEntry::started(
            42,
            Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            },
        ),
    )
    .unwrap();

    let report = Operator::new(OperationContext::new(&journal))
        .recover_interrupted()
        .unwrap();

    assert_eq!(fs::read_to_string(&destination).unwrap(), "recover me");
    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(report.outcomes[0].id, 42);
    assert_eq!(report.outcomes[0].status, OperationStatus::Completed);
    let entries = read_journal(&journal).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].id, 42);
    assert_eq!(entries[0].status, OperationStatus::Started);
    assert_eq!(entries[1].id, 42);
    assert_eq!(entries[1].status, OperationStatus::Completed);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovery_ignores_operations_with_terminal_status() {
    let root = unique_temp_dir("gfm-ops-recover-terminal");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "done").unwrap();
    append_journal(
        &journal,
        &JournalEntry::started(
            43,
            Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            },
        ),
    )
    .unwrap();
    append_journal(
        &journal,
        &JournalEntry::completed(
            43,
            Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            },
        ),
    )
    .unwrap();

    let report = Operator::new(OperationContext::new(&journal))
        .recover_interrupted()
        .unwrap();

    assert!(report.outcomes.is_empty());
    assert!(!destination.exists());
    assert_eq!(read_journal(&journal).unwrap().len(), 2);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovery_retries_classified_failed_operation_when_policy_allows_it() {
    let root = unique_temp_dir("gfm-ops-retry-failed");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    let operation = Operation::Copy {
        from: source.clone(),
        to: destination.clone(),
    };
    append_journal(&journal, &JournalEntry::started(44, operation.clone())).unwrap();
    append_journal(
        &journal,
        &JournalEntry::failed(
            44,
            operation.clone(),
            format!("{}: source does not exist", source.display()),
        ),
    )
    .unwrap();
    fs::write(&source, "arrived later").unwrap();

    let report = Operator::new(OperationContext::new(&journal))
        .recover_with_policy(OperationRecoveryPolicy {
            retry_failed: true,
            max_attempts: 2,
        })
        .unwrap();

    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(report.outcomes[0].id, 44);
    assert_eq!(report.outcomes[0].status, OperationStatus::Completed);
    assert_eq!(fs::read_to_string(&destination).unwrap(), "arrived later");
    let entries = read_journal(&journal).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[2].status, OperationStatus::Started);
    assert_eq!(entries[3].status, OperationStatus::Completed);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovery_does_not_retry_non_retryable_conflict_failures() {
    let root = unique_temp_dir("gfm-ops-retry-conflict");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "source").unwrap();
    fs::write(&destination, "destination").unwrap();
    let operation = Operation::Copy {
        from: source,
        to: destination.clone(),
    };
    append_journal(&journal, &JournalEntry::started(45, operation.clone())).unwrap();
    append_journal(
        &journal,
        &JournalEntry::failed(
            45,
            operation,
            format!("{}: destination already exists", destination.display()),
        ),
    )
    .unwrap();

    let report = Operator::new(OperationContext::new(&journal))
        .recover_with_policy(OperationRecoveryPolicy {
            retry_failed: true,
            max_attempts: 2,
        })
        .unwrap();

    assert!(report.outcomes.is_empty());
    assert_eq!(fs::read_to_string(&destination).unwrap(), "destination");
    assert_eq!(read_journal(&journal).unwrap().len(), 2);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn access_gate_prompts_before_mutating_destination_parent() {
    let root = unique_temp_dir("gfm-ops-access-prompt");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let protected = root.join("Documents");
    let destination = protected.join("destination.txt");
    fs::create_dir_all(&protected).unwrap();
    fs::write(&source, "source").unwrap();
    let gate = OperationAccessGate::new().with_decision(
        &protected,
        OperationAccessDecision::prompt("security-scoped bookmark required")
            .with_refresh_on_permission_change(true),
    );

    let err = Operator::new(OperationContext::new(&journal).with_access_gate(gate))
        .execute(Operation::Copy {
            from: source,
            to: destination.clone(),
        })
        .unwrap_err();

    assert!(matches!(err, GfmError::Permission { .. }));
    assert!(!destination.exists());
    let entries = read_journal(&journal).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].status, OperationStatus::Started);
    assert_eq!(entries[1].status, OperationStatus::Failed);
    assert!(entries[1]
        .message
        .as_deref()
        .unwrap()
        .contains("permission prompt"));
    assert!(entries[1]
        .message
        .as_deref()
        .unwrap()
        .contains("refresh-on-permission-change=true"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn access_gate_denies_before_destination_conflict_probe() {
    let root = unique_temp_dir("gfm-ops-access-before-conflict");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let protected = root.join("Documents");
    let destination = protected.join("destination.txt");
    fs::create_dir_all(&protected).unwrap();
    fs::write(&source, "source").unwrap();
    fs::write(&destination, "existing").unwrap();
    let gate = OperationAccessGate::new().with_decision(
        &protected,
        OperationAccessDecision::deny("unreachable volume network"),
    );

    let err = Operator::new(OperationContext::new(&journal).with_access_gate(gate))
        .execute(Operation::Copy {
            from: source,
            to: destination.clone(),
        })
        .unwrap_err();

    assert!(matches!(err, GfmError::Permission { .. }));
    assert!(
        err.to_string().contains("unreachable volume network"),
        "{err}"
    );
    assert!(
        !err.to_string().contains("destination already exists"),
        "{err}"
    );
    assert_eq!(fs::read_to_string(&destination).unwrap(), "existing");
    let entries = read_journal(&journal).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].status, OperationStatus::Started);
    assert_eq!(entries[1].status, OperationStatus::Failed);
    assert!(entries[1]
        .message
        .as_deref()
        .unwrap()
        .contains("unreachable volume network"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovery_does_not_retry_permission_prompt_failures() {
    let root = unique_temp_dir("gfm-ops-retry-permission");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "source").unwrap();
    let operation = Operation::Copy {
        from: source,
        to: destination.clone(),
    };
    append_journal(&journal, &JournalEntry::started(47, operation.clone())).unwrap();
    append_journal(
        &journal,
        &JournalEntry::failed(
            47,
            operation,
            "destination-parent requires a permission prompt before mutation".to_string(),
        ),
    )
    .unwrap();

    let report = Operator::new(OperationContext::new(&journal))
        .recover_with_policy(OperationRecoveryPolicy {
            retry_failed: true,
            max_attempts: 2,
        })
        .unwrap();

    assert!(report.outcomes.is_empty());
    assert!(!destination.exists());
    assert_eq!(read_journal(&journal).unwrap().len(), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn moves_files_with_replace_policy() {
    let root = unique_temp_dir("gfm-ops-move");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "new").unwrap();
    fs::write(&destination, "old").unwrap();

    let operator =
        Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace));
    operator
        .execute(Operation::Move {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert!(!source.exists());
    assert_eq!(fs::read_to_string(&destination).unwrap(), "new");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_move_replace_regular_file_preserves_existing_destination() {
    let root = unique_temp_dir("gfm-ops-move-replace-cancel");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "new").unwrap();
    fs::write(&destination, "old").unwrap();
    let cancellation = OperationCancellation::default();
    let operator = Operator::new(
        OperationContext::new(&journal)
            .with_conflict(ConflictPolicy::Replace)
            .with_cancellation(cancellation.clone()),
    );

    let err = operator
        .execute_with_progress(
            Operation::Move {
                from: source.clone(),
                to: destination.clone(),
            },
            |event| {
                if event.phase == OperationProgressPhase::Planned {
                    cancellation.cancel();
                }
            },
        )
        .unwrap_err();

    assert!(matches!(err, GfmError::Cancelled));
    assert_eq!(fs::read_to_string(&source).unwrap(), "new");
    assert_eq!(fs::read_to_string(&destination).unwrap(), "old");

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn move_replace_same_inode_removes_source_link_without_rewriting_destination() {
    use std::os::unix::fs::MetadataExt;

    let root = unique_temp_dir("gfm-ops-move-replace-same-inode");
    let journal = root.join("journal.log");
    let source = root.join("source.txt");
    let destination = root.join("destination.txt");
    fs::write(&source, "shared inode").unwrap();
    fs::hard_link(&source, &destination).unwrap();
    let before_destination = fs::metadata(&destination).unwrap();

    Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
        .execute(Operation::Move {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    let after_destination = fs::metadata(&destination).unwrap();
    assert!(!source.exists());
    assert_eq!(fs::read_to_string(&destination).unwrap(), "shared inode");
    assert_eq!(before_destination.ino(), after_destination.ino());
    assert_eq!(after_destination.nlink(), 1);

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn move_replace_symlink_uses_final_rename_without_predelete() {
    let root = unique_temp_dir("gfm-ops-move-replace-symlink");
    let journal = root.join("journal.log");
    let old_target = root.join("old-target.txt");
    let new_target = root.join("new-target.txt");
    let source = root.join("source-link");
    let destination = root.join("destination-link");
    fs::write(&old_target, "old").unwrap();
    fs::write(&new_target, "new").unwrap();
    std::os::unix::fs::symlink(&new_target, &source).unwrap();
    std::os::unix::fs::symlink(&old_target, &destination).unwrap();

    Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
        .execute(Operation::Move {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert!(!path_exists_or_symlink(&source));
    assert_eq!(fs::read_link(&destination).unwrap(), new_target);

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn cancelled_move_replace_symlink_preserves_existing_destination() {
    let root = unique_temp_dir("gfm-ops-move-replace-symlink-cancel");
    let journal = root.join("journal.log");
    let old_target = root.join("old-target.txt");
    let new_target = root.join("new-target.txt");
    let source = root.join("source-link");
    let destination = root.join("destination-link");
    fs::write(&old_target, "old").unwrap();
    fs::write(&new_target, "new").unwrap();
    std::os::unix::fs::symlink(&new_target, &source).unwrap();
    std::os::unix::fs::symlink(&old_target, &destination).unwrap();
    let cancellation = OperationCancellation::default();
    let operator = Operator::new(
        OperationContext::new(&journal)
            .with_conflict(ConflictPolicy::Replace)
            .with_cancellation(cancellation.clone()),
    );

    let err = operator
        .execute_with_progress(
            Operation::Move {
                from: source.clone(),
                to: destination.clone(),
            },
            |event| {
                if event.phase == OperationProgressPhase::Planned {
                    cancellation.cancel();
                }
            },
        )
        .unwrap_err();

    assert!(matches!(err, GfmError::Cancelled));
    assert_eq!(fs::read_link(&source).unwrap(), new_target);
    assert_eq!(fs::read_link(&destination).unwrap(), old_target);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn move_replace_directory_uses_final_rename_without_predelete() {
    let root = unique_temp_dir("gfm-ops-move-replace-directory");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested").join("new.txt"), "new").unwrap();
    fs::create_dir_all(destination.join("nested")).unwrap();
    fs::write(destination.join("nested").join("old.txt"), "old").unwrap();

    Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
        .execute(Operation::Move {
            from: source.clone(),
            to: destination.clone(),
        })
        .unwrap();

    assert!(!path_exists_or_symlink(&source));
    assert_eq!(
        fs::read_to_string(destination.join("nested").join("new.txt")).unwrap(),
        "new"
    );
    assert!(!destination.join("nested").join("old.txt").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_move_replace_directory_preserves_existing_destination() {
    let root = unique_temp_dir("gfm-ops-move-replace-directory-cancel");
    let journal = root.join("journal.log");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested").join("new.txt"), "new").unwrap();
    fs::create_dir_all(destination.join("nested")).unwrap();
    fs::write(destination.join("nested").join("old.txt"), "old").unwrap();
    let cancellation = OperationCancellation::default();
    let operator = Operator::new(
        OperationContext::new(&journal)
            .with_conflict(ConflictPolicy::Replace)
            .with_cancellation(cancellation.clone()),
    );

    let err = operator
        .execute_with_progress(
            Operation::Move {
                from: source.clone(),
                to: destination.clone(),
            },
            |event| {
                if event.phase == OperationProgressPhase::Planned {
                    cancellation.cancel();
                }
            },
        )
        .unwrap_err();

    assert!(matches!(err, GfmError::Cancelled));
    assert_eq!(
        fs::read_to_string(source.join("nested").join("new.txt")).unwrap(),
        "new"
    );
    assert_eq!(
        fs::read_to_string(destination.join("nested").join("old.txt")).unwrap(),
        "old"
    );
    assert!(!destination.join("nested").join("new.txt").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deletes_files_and_directories() {
    let root = unique_temp_dir("gfm-ops-delete");
    let journal = root.join("journal.log");
    let target = root.join("target");
    fs::create_dir_all(target.join("nested")).unwrap();
    fs::write(target.join("nested").join("file.txt"), "gone").unwrap();

    let operator = Operator::new(OperationContext::new(&journal));
    operator
        .execute(Operation::Delete {
            path: target.clone(),
        })
        .unwrap();

    assert!(!target.exists());
    fs::remove_dir_all(root).unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("{prefix}-{}", now_nanos()));
    fs::create_dir_all(&path).unwrap();
    path
}
