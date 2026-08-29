use crate::access::{preflight_access_scope, preflight_volume_access_scope, ScopedAccessGuard};
use crate::runtime::run_volume_task_cancellable;
use crate::{parse_u64_arg, parse_usize_arg, path_volume, required_path};
use gfm_index::{
    ContentArchiveCleanupPolicy, ContentArchiveManifest, ContentArchiveManifestEntry,
    ContentMergeTier,
};
use gfm_jobs::Priority;
use gfm_mac::AccessIntent;
use gfm_store::{
    content_manifest_promotion_journal_path, plan_content_manifest_promotion_recovery,
    plan_content_manifest_recovery, promote_content_archive_manifest, recover_content_manifest,
    recover_content_manifest_promotion, ContentArchiveHealth, ContentManifestPromotionJournal,
    MmapContentSet,
};
use gfm_types::{GfmError, Result};
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "content-manifest-write" => {
            let output = required_path(
                args.next(),
                "content-manifest-write requires a manifest path",
            )?;
            let archives = args
                .map(|spec| parse_content_manifest_archive_spec(&spec))
                .collect::<Result<Vec<_>>>()?;
            eprintln!("{}", run_manifest_write(output, archives)?);
        }
        "content-manifest-inspect" => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-inspect requires a manifest path",
            )?;
            for line in run_manifest_inspect(manifest_path)? {
                println!("{line}");
            }
        }
        "content-manifest-recovery-plan" => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-recovery-plan requires a manifest path",
            )?;
            let discovered = args
                .map(|spec| parse_content_manifest_archive_spec(&spec))
                .collect::<Result<Vec<_>>>()?;
            for line in run_manifest_recovery_plan(manifest_path, discovered)? {
                println!("{line}");
            }
        }
        "content-manifest-recover" => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-recover requires a manifest path",
            )?;
            let quarantine = required_path(
                args.next(),
                "content-manifest-recover requires a quarantine directory",
            )?;
            let discovered = args
                .map(|spec| parse_content_manifest_archive_spec(&spec))
                .collect::<Result<Vec<_>>>()?;
            for line in run_manifest_recover(manifest_path, quarantine, discovered)? {
                println!("{line}");
            }
        }
        "content-manifest-promote" => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-promote requires a manifest path",
            )?;
            let new_archive = args.next().ok_or_else(|| {
                GfmError::Format(
                    "content-manifest-promote requires a hot:path, warm:path, or cold:path archive"
                        .to_string(),
                )
            })?;
            let new_archive = parse_content_manifest_archive_spec(&new_archive)?;
            let retired_paths = args.map(PathBuf::from).collect::<Vec<_>>();
            let (summary, lines) =
                run_manifest_promotion(manifest_path, new_archive, retired_paths)?;
            eprintln!("{summary}");
            for line in lines {
                println!("{line}");
            }
        }
        "content-manifest-promotion-recovery-plan" => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-promotion-recovery-plan requires a manifest path",
            )?;
            println!("{}", run_manifest_promotion_recovery_plan(manifest_path)?);
        }
        "content-manifest-promotion-recover" => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-promotion-recover requires a manifest path",
            )?;
            for line in run_manifest_promotion_recover(manifest_path)? {
                println!("{line}");
            }
        }
        "content-manifest-cleanup" => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-cleanup requires a manifest path",
            )?;
            let candidates = args.map(PathBuf::from).collect::<Vec<_>>();
            if candidates.is_empty() {
                return Err(GfmError::Format(
                    "content-manifest-cleanup requires at least one candidate archive".to_string(),
                ));
            }
            let (summary, lines) = run_manifest_cleanup(manifest_path, candidates)?;
            eprintln!("{summary}");
            for line in lines {
                println!("{line}");
            }
        }
        "content-cleanup-plan" => {
            let manifest_path =
                required_path(args.next(), "content-cleanup-plan requires a manifest path")?;
            let min_retired_archives = parse_usize_arg(
                args.next(),
                "content-cleanup-plan requires min-retired-archives",
            )?;
            let min_retired_bytes = parse_u64_arg(
                args.next(),
                "content-cleanup-plan requires min-retired-bytes",
            )?;
            let max_cleanup_archives = parse_usize_arg(
                args.next(),
                "content-cleanup-plan requires max-cleanup-archives",
            )?;
            let candidates = args.map(PathBuf::from).collect::<Vec<_>>();
            let (summary, lines) = run_content_cleanup_plan(
                manifest_path,
                candidates,
                ContentArchiveCleanupPolicy {
                    min_retired_archives,
                    min_retired_bytes,
                    max_cleanup_archives,
                },
            )?;
            eprintln!("{summary}");
            for line in lines {
                println!("{line}");
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn run_manifest_cleanup(
    manifest_path: PathBuf,
    candidates: Vec<PathBuf>,
) -> Result<(String, Vec<String>)> {
    const WORKER: &str = "content manifest cleanup";
    preflight_manifest_cleanup_volumes(&manifest_path, &candidates, true, WORKER)?;
    let volume = path_volume(&manifest_path);
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_manifest_cleanup_access(&manifest_path, &candidates, true, WORKER)?;
        cancellation.check()?;
        render_manifest_cleanup(&manifest_path, &candidates)
    })
}

fn run_content_cleanup_plan(
    manifest_path: PathBuf,
    candidates: Vec<PathBuf>,
    policy: ContentArchiveCleanupPolicy,
) -> Result<(String, Vec<String>)> {
    const WORKER: &str = "content cleanup plan";
    const ACTIVE_ARCHIVE_WORKER: &str = "content cleanup plan active archive";
    preflight_manifest_cleanup_volumes(&manifest_path, &candidates, false, WORKER)?;
    let volume = path_volume(&manifest_path);
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_manifest_cleanup_access(&manifest_path, &candidates, false, WORKER)?;
        let manifest = ContentArchiveManifest::read(&manifest_path)?;
        let active_archive_paths = manifest.resolved_archive_paths(&manifest_path);
        preflight_manifest_cleanup_archive_volumes(&active_archive_paths, ACTIVE_ARCHIVE_WORKER)?;
        cancellation.check()?;
        let _active_archive_access = retain_manifest_promotion_recovery_archive_access(
            &active_archive_paths,
            ACTIVE_ARCHIVE_WORKER,
        )?;
        cancellation.check()?;
        render_cleanup_plan(&manifest_path, &candidates, &policy)
    })
}

fn render_manifest_cleanup(
    manifest_path: &Path,
    candidates: &[PathBuf],
) -> Result<(String, Vec<String>)> {
    let manifest = ContentArchiveManifest::read(manifest_path)?;
    let report = manifest.cleanup_inactive_archives(manifest_path, candidates)?;
    let summary = format!(
        "content-manifest-cleanup\tremoved={}\tactive={}\tmissing={}",
        report.removed_archives.len(),
        report.active_archives.len(),
        report.missing_archives.len()
    );
    let mut lines = Vec::new();
    for path in report.removed_archives {
        lines.push(format!("removed\t{}", path.display()));
    }
    for path in report.active_archives {
        lines.push(format!("active\t{}", path.display()));
    }
    for path in report.missing_archives {
        lines.push(format!("missing\t{}", path.display()));
    }
    Ok((summary, lines))
}

fn render_cleanup_plan(
    manifest_path: &Path,
    candidates: &[PathBuf],
    policy: &ContentArchiveCleanupPolicy,
) -> Result<(String, Vec<String>)> {
    let manifest = ContentArchiveManifest::read(manifest_path)?;
    let plan = manifest.plan_inactive_archive_cleanup(manifest_path, candidates, policy)?;
    let summary = format!(
        "content-cleanup-plan\taction={:?}\tcleanup={}\tdeferred={}\tactive={}\tmissing={}\tactive-bytes={}\tcleanup-bytes={}\tdeferred-bytes={}",
        plan.action,
        plan.cleanup_archives.len(),
        plan.deferred_archives.len(),
        plan.active_archives.len(),
        plan.missing_archives.len(),
        plan.active_bytes,
        plan.cleanup_bytes,
        plan.deferred_bytes
    );
    let mut lines = Vec::new();
    for path in plan.cleanup_archives {
        lines.push(format!("cleanup\t{}", path.display()));
    }
    for path in plan.deferred_archives {
        lines.push(format!("defer\t{}", path.display()));
    }
    for path in plan.active_archives {
        lines.push(format!("active\t{}", path.display()));
    }
    for path in plan.missing_archives {
        lines.push(format!("missing\t{}", path.display()));
    }
    Ok((summary, lines))
}

fn preflight_manifest_cleanup_volumes(
    manifest_path: &Path,
    candidates: &[PathBuf],
    removes_candidates: bool,
    worker: &str,
) -> Result<()> {
    preflight_volume_access_scope(manifest_path, AccessIntent::Read, worker)?;
    for candidate in candidates {
        let path = resolve_manifest_path(manifest_path, candidate);
        let (path, intent) = if removes_candidates {
            (write_probe_path(&path), AccessIntent::Write)
        } else {
            (existing_read_probe_path(&path)?, AccessIntent::Read)
        };
        preflight_volume_access_scope(path, intent, "content manifest cleanup candidate")?;
    }
    Ok(())
}

fn preflight_manifest_cleanup_archive_volumes(paths: &[PathBuf], worker: &str) -> Result<()> {
    for path in paths {
        preflight_volume_access_scope(path, AccessIntent::Read, worker)?;
    }
    Ok(())
}

fn retain_manifest_write_access(
    manifest_path: &Path,
    archives: &[ContentArchiveManifestEntry],
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = vec![preflight_access_scope(
        write_probe_path(manifest_path),
        AccessIntent::Write,
        "content manifest write",
    )?];
    for archive in archives {
        guards.push(preflight_access_scope(
            &resolve_manifest_path(manifest_path, &archive.path),
            AccessIntent::Read,
            "content manifest write archive",
        )?);
    }
    Ok(guards)
}

fn preflight_manifest_write_volumes(
    manifest_path: &Path,
    archives: &[ContentArchiveManifestEntry],
) -> Result<()> {
    preflight_volume_access_scope(
        write_probe_path(manifest_path),
        AccessIntent::Write,
        "content manifest write",
    )?;
    for archive in archives {
        preflight_volume_access_scope(
            &resolve_manifest_path(manifest_path, &archive.path),
            AccessIntent::Read,
            "content manifest write archive",
        )?;
    }
    Ok(())
}

fn run_manifest_write(
    manifest_path: PathBuf,
    archives: Vec<ContentArchiveManifestEntry>,
) -> Result<String> {
    const WORKER: &str = "content manifest write";
    preflight_manifest_write_volumes(&manifest_path, &archives)?;
    let volume = path_volume(write_probe_path(&manifest_path));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_manifest_write_access(&manifest_path, &archives)?;
        cancellation.check()?;
        let manifest = ContentArchiveManifest::new(archives)?;
        manifest.write(&manifest_path)?;
        Ok(format!(
            "content-manifest\tarchives={}",
            manifest.archives.len()
        ))
    })
}

fn retain_manifest_inspect_archive_access<'a>(
    archive_paths: impl Iterator<Item = &'a PathBuf>,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::new();
    for path in archive_paths {
        guards.push(preflight_access_scope(
            path,
            AccessIntent::Read,
            "content manifest inspect archive",
        )?);
    }
    Ok(guards)
}

fn run_manifest_inspect(manifest_path: PathBuf) -> Result<Vec<String>> {
    preflight_volume_access_scope(
        &manifest_path,
        AccessIntent::Read,
        "content manifest inspect",
    )?;
    let volume = path_volume(&manifest_path);
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "content manifest inspect",
        move |cancellation| {
            cancellation.check()?;
            let _manifest_access = preflight_access_scope(
                &manifest_path,
                AccessIntent::Read,
                "content manifest inspect",
            )?;
            let manifest = ContentArchiveManifest::read(&manifest_path)?;
            let paths = manifest.resolved_archive_paths(&manifest_path);
            for path in &paths {
                preflight_volume_access_scope(
                    path,
                    AccessIntent::Read,
                    "content manifest inspect archive",
                )?;
            }
            cancellation.check()?;
            let _archive_access = retain_manifest_inspect_archive_access(paths.iter())?;
            cancellation.check()?;
            let set = MmapContentSet::open(&paths)?;
            let mut lines = vec![format!(
                "content-manifest\tarchives={}\tterms={}\tbytes={}",
                set.archive_count(),
                set.indexed_terms(),
                set.mapped_len()
            )];
            for (entry, path) in manifest.archives.iter().zip(paths) {
                lines.push(format!(
                    "archive\t{}\t{}\t{}",
                    content_tier_name(entry.tier),
                    entry.path.display(),
                    path.display()
                ));
            }
            Ok(lines)
        },
    )
}

fn retain_manifest_recovery_plan_access<'a>(
    manifest_path: &Path,
    discovered: impl Iterator<Item = &'a ContentArchiveManifestEntry>,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = vec![preflight_access_scope(
        existing_read_probe_path(manifest_path)?,
        AccessIntent::Read,
        "content manifest recovery plan",
    )?];
    for entry in discovered {
        guards.push(preflight_access_scope(
            existing_read_probe_path(&resolve_manifest_path(manifest_path, &entry.path))?,
            AccessIntent::Read,
            "content manifest recovery discovered archive",
        )?);
    }
    Ok(guards)
}

fn preflight_manifest_recovery_plan_volumes(
    manifest_path: &Path,
    discovered: &[ContentArchiveManifestEntry],
) -> Result<()> {
    preflight_volume_access_scope(
        existing_read_probe_path(manifest_path)?,
        AccessIntent::Read,
        "content manifest recovery plan",
    )?;
    for entry in discovered {
        preflight_volume_access_scope(
            existing_read_probe_path(&resolve_manifest_path(manifest_path, &entry.path))?,
            AccessIntent::Read,
            "content manifest recovery discovered archive",
        )?;
    }
    Ok(())
}

fn run_manifest_recovery_plan(
    manifest_path: PathBuf,
    discovered: Vec<ContentArchiveManifestEntry>,
) -> Result<Vec<String>> {
    preflight_manifest_recovery_plan_volumes(&manifest_path, &discovered)?;
    let volume = path_volume(existing_read_probe_path(&manifest_path)?);
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "content manifest recovery plan",
        move |cancellation| {
            cancellation.check()?;
            let _access = retain_manifest_recovery_plan_access(&manifest_path, discovered.iter())?;
            cancellation.check()?;
            let plan = plan_content_manifest_recovery(&manifest_path, &discovered);
            let mut lines = vec![plan.as_tsv()];
            lines.extend(format_content_archive_health(
                "invalid",
                &plan.invalid_archives,
            ));
            Ok(lines)
        },
    )
}

fn preflight_manifest_recovery_volumes(
    manifest_path: &Path,
    quarantine: &Path,
    discovered: &[ContentArchiveManifestEntry],
) -> Result<()> {
    preflight_manifest_recovery_plan_volumes(manifest_path, discovered)?;
    preflight_volume_access_scope(
        write_probe_path(manifest_path),
        AccessIntent::Write,
        "content manifest recovery manifest",
    )?;
    preflight_volume_access_scope(
        write_probe_path(quarantine),
        AccessIntent::Write,
        "content manifest recovery quarantine",
    )
}

fn run_manifest_recover(
    manifest_path: PathBuf,
    quarantine: PathBuf,
    discovered: Vec<ContentArchiveManifestEntry>,
) -> Result<Vec<String>> {
    preflight_manifest_recovery_volumes(&manifest_path, &quarantine, &discovered)?;
    let volume = path_volume(&manifest_path)
        .or_else(|| path_volume(write_probe_path(&manifest_path)))
        .or_else(|| path_volume(write_probe_path(&quarantine)));
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "content manifest recovery",
        move |cancellation| {
            cancellation.check()?;
            let _access =
                retain_manifest_recovery_access(&manifest_path, &quarantine, discovered.iter())?;
            cancellation.check()?;
            let report = recover_content_manifest(&manifest_path, &discovered, &quarantine)?;
            let mut lines = vec![
                report.before.as_tsv(),
                format!(
                    "content-manifest-recovery\twrote-manifest={}\tquarantined-manifest={}",
                    report.wrote_manifest,
                    report
                        .quarantined_manifest_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "-".to_string())
                ),
                report.after.as_tsv(),
            ];
            lines.extend(format_content_archive_health(
                "invalid-before",
                &report.before.invalid_archives,
            ));
            Ok(lines)
        },
    )
}

fn retain_manifest_recovery_access<'a>(
    manifest_path: &Path,
    quarantine: &Path,
    discovered: impl Iterator<Item = &'a ContentArchiveManifestEntry>,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = retain_manifest_recovery_plan_access(manifest_path, discovered)?;
    guards.push(preflight_access_scope(
        write_probe_path(manifest_path),
        AccessIntent::Write,
        "content manifest recovery manifest",
    )?);
    guards.push(preflight_access_scope(
        write_probe_path(quarantine),
        AccessIntent::Write,
        "content manifest recovery quarantine",
    )?);
    Ok(guards)
}

fn retain_manifest_promotion_access(
    manifest_path: &Path,
    new_archive: &ContentArchiveManifestEntry,
    retired_paths: &[PathBuf],
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = vec![
        preflight_access_scope(
            manifest_path,
            AccessIntent::Read,
            "content manifest promotion manifest",
        )?,
        preflight_access_scope(
            write_probe_path(manifest_path),
            AccessIntent::Write,
            "content manifest promotion manifest",
        )?,
        preflight_access_scope(
            &resolve_manifest_path(manifest_path, &new_archive.path),
            AccessIntent::Read,
            "content manifest promotion archive",
        )?,
    ];
    for path in retired_paths {
        guards.push(preflight_access_scope(
            existing_read_probe_path(&resolve_manifest_path(manifest_path, path))?,
            AccessIntent::Read,
            "content manifest promotion retirement",
        )?);
    }
    Ok(guards)
}

fn preflight_manifest_promotion_volumes(
    manifest_path: &Path,
    new_archive: &ContentArchiveManifestEntry,
    retired_paths: &[PathBuf],
) -> Result<()> {
    preflight_volume_access_scope(
        manifest_path,
        AccessIntent::Read,
        "content manifest promotion manifest",
    )?;
    preflight_volume_access_scope(
        write_probe_path(manifest_path),
        AccessIntent::Write,
        "content manifest promotion manifest",
    )?;
    preflight_volume_access_scope(
        &resolve_manifest_path(manifest_path, &new_archive.path),
        AccessIntent::Read,
        "content manifest promotion archive",
    )?;
    for path in retired_paths {
        preflight_volume_access_scope(
            existing_read_probe_path(&resolve_manifest_path(manifest_path, path))?,
            AccessIntent::Read,
            "content manifest promotion retirement",
        )?;
    }
    Ok(())
}

fn run_manifest_promotion(
    manifest_path: PathBuf,
    new_archive: ContentArchiveManifestEntry,
    retired_paths: Vec<PathBuf>,
) -> Result<(String, Vec<String>)> {
    const WORKER: &str = "content manifest promotion";
    preflight_manifest_promotion_volumes(&manifest_path, &new_archive, &retired_paths)?;
    let volume = path_volume(&manifest_path);
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access =
            retain_manifest_promotion_access(&manifest_path, &new_archive, &retired_paths)?;
        cancellation.check()?;
        let promotion =
            promote_content_archive_manifest(&manifest_path, new_archive, &retired_paths)?;
        let summary = format!(
            "content-manifest-promoted\tarchives={}\tretired={}\tmissing-retirements={}",
            promotion.manifest.archives.len(),
            promotion.retired_archives.len(),
            promotion.missing_retirements.len()
        );
        let mut lines = Vec::new();
        for path in promotion.retired_archives {
            lines.push(format!("retire\t{}", path.display()));
        }
        for path in promotion.missing_retirements {
            lines.push(format!("missing-retirement\t{}", path.display()));
        }
        Ok((summary, lines))
    })
}

fn retain_manifest_promotion_recovery_plan_access(
    manifest_path: &Path,
) -> Result<Vec<ScopedAccessGuard>> {
    let journal_path = content_manifest_promotion_journal_path(manifest_path);
    Ok(vec![
        preflight_access_scope(
            manifest_path,
            AccessIntent::Read,
            "content manifest promotion recovery plan",
        )?,
        preflight_access_scope(
            existing_read_probe_path(&journal_path)?,
            AccessIntent::Read,
            "content manifest promotion recovery journal",
        )?,
    ])
}

fn preflight_manifest_promotion_recovery_plan_volumes(manifest_path: &Path) -> Result<()> {
    let journal_path = content_manifest_promotion_journal_path(manifest_path);
    preflight_volume_access_scope(
        manifest_path,
        AccessIntent::Read,
        "content manifest promotion recovery plan",
    )?;
    preflight_volume_access_scope(
        existing_read_probe_path(&journal_path)?,
        AccessIntent::Read,
        "content manifest promotion recovery journal",
    )
}

fn preflight_manifest_promotion_recovery_volumes(manifest_path: &Path) -> Result<()> {
    let journal_path = content_manifest_promotion_journal_path(manifest_path);
    preflight_volume_access_scope(
        manifest_path,
        AccessIntent::Read,
        "content manifest promotion recovery",
    )?;
    preflight_volume_access_scope(
        write_probe_path(manifest_path),
        AccessIntent::Write,
        "content manifest promotion recovery",
    )?;
    preflight_volume_access_scope(
        existing_read_probe_path(&journal_path)?,
        AccessIntent::Read,
        "content manifest promotion recovery journal",
    )?;
    preflight_volume_access_scope(
        write_probe_path(&journal_path),
        AccessIntent::Write,
        "content manifest promotion recovery journal",
    )
}

fn preflight_manifest_promotion_recovery_archives(paths: &[PathBuf], worker: &str) -> Result<()> {
    for path in paths {
        preflight_volume_access_scope(path, AccessIntent::Read, worker)?;
    }
    Ok(())
}

fn retain_manifest_promotion_recovery_archive_access(
    paths: &[PathBuf],
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::new();
    for path in paths {
        guards.push(preflight_access_scope(path, AccessIntent::Read, worker)?);
    }
    Ok(guards)
}

fn run_manifest_promotion_recovery_plan(manifest_path: PathBuf) -> Result<String> {
    const WORKER: &str = "content manifest promotion recovery plan";
    const ARCHIVE_WORKER: &str = "content manifest promotion recovery archive";
    preflight_manifest_promotion_recovery_plan_volumes(&manifest_path)?;
    let volume = path_volume(&manifest_path);
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_manifest_promotion_recovery_plan_access(&manifest_path)?;
        let journal_path = content_manifest_promotion_journal_path(&manifest_path);
        let archive_paths = if manifest_path_exists(&journal_path, "promotion journal")? {
            let journal = ContentManifestPromotionJournal::read(&journal_path)?;
            promotion_recovery_archive_paths(&manifest_path, &journal)?
        } else {
            Vec::new()
        };
        preflight_manifest_promotion_recovery_archives(&archive_paths, ARCHIVE_WORKER)?;
        cancellation.check()?;
        let _archive_access =
            retain_manifest_promotion_recovery_archive_access(&archive_paths, ARCHIVE_WORKER)?;
        cancellation.check()?;
        Ok(plan_content_manifest_promotion_recovery(manifest_path).as_tsv())
    })
}

fn run_manifest_promotion_recover(manifest_path: PathBuf) -> Result<Vec<String>> {
    const WORKER: &str = "content manifest promotion recovery";
    const ARCHIVE_WORKER: &str = "content manifest promotion recovery archive";
    preflight_manifest_promotion_recovery_volumes(&manifest_path)?;
    let volume =
        path_volume(&manifest_path).or_else(|| path_volume(write_probe_path(&manifest_path)));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_manifest_promotion_recovery_access(&manifest_path)?;
        let journal_path = content_manifest_promotion_journal_path(&manifest_path);
        let archive_paths = if manifest_path_exists(&journal_path, "promotion journal")? {
            let journal = ContentManifestPromotionJournal::read(&journal_path)?;
            promotion_recovery_archive_paths(&manifest_path, &journal)?
        } else {
            Vec::new()
        };
        preflight_manifest_promotion_recovery_archives(&archive_paths, ARCHIVE_WORKER)?;
        cancellation.check()?;
        let _archive_access =
            retain_manifest_promotion_recovery_archive_access(&archive_paths, ARCHIVE_WORKER)?;
        cancellation.check()?;
        let recovery = recover_content_manifest_promotion(manifest_path)?;
        Ok(vec![
            recovery.before.as_tsv(),
            format!(
                "content-manifest-promotion-recovery\tcompleted-promotion={}\tremoved-journal={}",
                recovery.completed_promotion, recovery.removed_journal
            ),
            recovery.after.as_tsv(),
        ])
    })
}

fn promotion_recovery_archive_paths(
    manifest_path: &Path,
    journal: &ContentManifestPromotionJournal,
) -> Result<Vec<PathBuf>> {
    let promotion = journal.previous.promote_archive(
        manifest_path,
        journal.new_archive.clone(),
        &journal.retired_paths,
    )?;
    Ok(promotion.manifest.resolved_archive_paths(manifest_path))
}

fn retain_manifest_promotion_recovery_access(
    manifest_path: &Path,
) -> Result<Vec<ScopedAccessGuard>> {
    let journal_path = content_manifest_promotion_journal_path(manifest_path);
    Ok(vec![
        preflight_access_scope(
            manifest_path,
            AccessIntent::Read,
            "content manifest promotion recovery",
        )?,
        preflight_access_scope(
            write_probe_path(manifest_path),
            AccessIntent::Write,
            "content manifest promotion recovery",
        )?,
        preflight_access_scope(
            existing_read_probe_path(&journal_path)?,
            AccessIntent::Read,
            "content manifest promotion recovery journal",
        )?,
        preflight_access_scope(
            write_probe_path(&journal_path),
            AccessIntent::Write,
            "content manifest promotion recovery journal",
        )?,
    ])
}

fn retain_manifest_cleanup_access(
    manifest_path: &Path,
    candidates: &[PathBuf],
    removes_candidates: bool,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = vec![preflight_access_scope(
        manifest_path,
        AccessIntent::Read,
        worker,
    )?];
    for candidate in candidates {
        let path = resolve_manifest_path(manifest_path, candidate);
        let (path, intent) = if removes_candidates {
            (write_probe_path(&path), AccessIntent::Write)
        } else {
            (existing_read_probe_path(&path)?, AccessIntent::Read)
        };
        guards.push(preflight_access_scope(
            path,
            intent,
            "content manifest cleanup candidate",
        )?);
    }
    Ok(guards)
}

fn resolve_manifest_path(manifest_path: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    manifest_path
        .parent()
        .map(|parent| parent.join(path))
        .unwrap_or_else(|| path.to_path_buf())
}

fn write_probe_path(path: &Path) -> &Path {
    if path.is_dir() {
        return path;
    }
    crate::parent_or_cwd(path)
}

fn manifest_path_exists(path: &Path, label: &str) -> Result<bool> {
    path.try_exists().map_err(|err| {
        GfmError::io(
            path,
            format!("manifest {label} existence unavailable: {err}"),
        )
    })
}

fn existing_read_probe_path(path: &Path) -> Result<&Path> {
    if path.try_exists().map_err(|err| {
        GfmError::io(
            path,
            format!("manifest read path existence unavailable: {err}"),
        )
    })? {
        return Ok(path);
    }
    Ok(write_probe_path(path))
}

fn parse_content_manifest_archive_spec(value: &str) -> Result<ContentArchiveManifestEntry> {
    let (tier, path) = value.split_once(':').ok_or_else(|| {
        GfmError::Format(format!(
            "content manifest archive `{value}` must be formatted as hot:path, warm:path, or cold:path"
        ))
    })?;
    if path.is_empty() {
        return Err(GfmError::Format(format!(
            "content manifest archive `{value}` has an empty path"
        )));
    }
    Ok(ContentArchiveManifestEntry {
        tier: parse_content_tier(tier)?,
        path: PathBuf::from(path),
    })
}

fn parse_content_tier(value: &str) -> Result<ContentMergeTier> {
    match value {
        "hot" => Ok(ContentMergeTier::Hot),
        "warm" => Ok(ContentMergeTier::Warm),
        "cold" => Ok(ContentMergeTier::Cold),
        other => Err(GfmError::Format(format!(
            "content archive tier must be hot, warm, or cold; got `{other}`"
        ))),
    }
}

fn content_tier_name(tier: ContentMergeTier) -> &'static str {
    match tier {
        ContentMergeTier::Hot => "hot",
        ContentMergeTier::Warm => "warm",
        ContentMergeTier::Cold => "cold",
    }
}

fn format_content_archive_health(label: &str, archives: &[ContentArchiveHealth]) -> Vec<String> {
    archives
        .iter()
        .map(|archive| {
            format!(
                "{}\t{}\t{}\t{}",
                label,
                content_tier_name(archive.entry.tier),
                archive.resolved_path.display(),
                archive.detail.as_deref().unwrap_or("-")
            )
        })
        .collect()
}
