#[cfg(test)]
use crate::access::preflight_access_scope_checked;
use crate::access::{
    preflight_access_scope_checked_with_volume_report, preflight_volume_access_scope_with_report,
    ScopedAccessGuard,
};
use crate::runtime::run_volume_task_cancellable;
use crate::{parse_u64_arg, parse_usize_arg, required_path};
use gfm_index::{
    ContentArchiveCleanupPolicy, ContentArchiveManifest, ContentArchiveManifestEntry,
    ContentMergeTier,
};
use gfm_jobs::Priority;
use gfm_mac::{AccessIntent, VolumeDiscoveryReport};
use gfm_store::{
    cleanup_inactive_content_archives_checked, content_manifest_promotion_journal_path,
    plan_content_manifest_promotion_recovery_checked, plan_content_manifest_recovery_checked,
    plan_inactive_content_archive_cleanup_checked, promote_content_archive_manifest_checked,
    recover_content_manifest_checked, recover_content_manifest_promotion_checked,
    ContentArchiveHealth, ContentManifestPromotionJournal, MmapContentSet,
};
use gfm_types::{GfmError, Result, VolumeId};
use std::fs;
use std::io;
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
    let access_reports =
        ManifestAccessReports::cleanup_for_paths(&manifest_path, &candidates, true, WORKER)?;
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        render_manifest_cleanup(&manifest_path, &candidates, || cancellation.check())
    })
}

fn run_content_cleanup_plan(
    manifest_path: PathBuf,
    candidates: Vec<PathBuf>,
    policy: ContentArchiveCleanupPolicy,
) -> Result<(String, Vec<String>)> {
    const WORKER: &str = "content cleanup plan";
    const ACTIVE_ARCHIVE_WORKER: &str = "content cleanup plan active archive";
    let access_reports =
        ManifestAccessReports::cleanup_for_paths(&manifest_path, &candidates, false, WORKER)?;
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(|| cancellation.check())?;
        let manifest =
            ContentArchiveManifest::read_checked(&manifest_path, || cancellation.check())?;
        let active_archive_paths = manifest.resolved_archive_paths(&manifest_path);
        let active_archive_access_reports = ManifestAccessReports::read_paths_checked(
            &active_archive_paths,
            ACTIVE_ARCHIVE_WORKER,
            || cancellation.check(),
        )?;
        active_archive_access_reports.preflight_volumes()?;
        cancellation.check()?;
        let _active_archive_access =
            active_archive_access_reports.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        render_cleanup_plan(&manifest_path, &candidates, &policy, || {
            cancellation.check()
        })
    })
}

fn render_manifest_cleanup(
    manifest_path: &Path,
    candidates: &[PathBuf],
    check_control: impl FnMut() -> Result<()>,
) -> Result<(String, Vec<String>)> {
    let report =
        cleanup_inactive_content_archives_checked(manifest_path, candidates, check_control)?;
    let summary = format!(
        "content-manifest-cleanup\tremoved={}\tactive={}\tmissing={}",
        report.removed_archives.len(),
        report.active_archives.len(),
        report.missing_archives.len()
    );
    let mut lines = Vec::new();
    for path in report.removed_archives {
        lines.push(format!("removed\t{}", escape_manifest_tsv_path(&path)));
    }
    for path in report.active_archives {
        lines.push(format!("active\t{}", escape_manifest_tsv_path(&path)));
    }
    for path in report.missing_archives {
        lines.push(format!("missing\t{}", escape_manifest_tsv_path(&path)));
    }
    Ok((summary, lines))
}

fn render_cleanup_plan(
    manifest_path: &Path,
    candidates: &[PathBuf],
    policy: &ContentArchiveCleanupPolicy,
    check_control: impl FnMut() -> Result<()>,
) -> Result<(String, Vec<String>)> {
    let plan = plan_inactive_content_archive_cleanup_checked(
        manifest_path,
        candidates,
        policy,
        check_control,
    )?;
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
        lines.push(format!("cleanup\t{}", escape_manifest_tsv_path(&path)));
    }
    for path in plan.deferred_archives {
        lines.push(format!("defer\t{}", escape_manifest_tsv_path(&path)));
    }
    for path in plan.active_archives {
        lines.push(format!("active\t{}", escape_manifest_tsv_path(&path)));
    }
    for path in plan.missing_archives {
        lines.push(format!("missing\t{}", escape_manifest_tsv_path(&path)));
    }
    Ok((summary, lines))
}

#[derive(Clone)]
struct ManifestAccessReport {
    path: PathBuf,
    intent: AccessIntent,
    worker: &'static str,
    volume_report: VolumeDiscoveryReport,
}

impl ManifestAccessReport {
    fn new_checked(
        path: PathBuf,
        intent: AccessIntent,
        worker: &'static str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let volume_report =
            VolumeDiscoveryReport::for_containing_path_policy_checked(&path, &mut check_control)?;
        check_control()?;
        Ok(Self {
            path,
            intent,
            worker,
            volume_report,
        })
    }

    fn preflight_volume(&self) -> Result<()> {
        preflight_volume_access_scope_with_report(
            &self.path,
            self.intent,
            self.worker,
            &self.volume_report,
        )
    }

    fn access_checked(
        &self,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<ScopedAccessGuard> {
        preflight_access_scope_checked_with_volume_report(
            &self.path,
            self.intent,
            self.worker,
            &self.volume_report,
            &mut check_control,
        )
    }

    fn volume(&self) -> Option<VolumeId> {
        self.volume_report
            .volume_for_path(&self.path)
            .map(|volume| volume.id)
    }
}

#[derive(Clone)]
struct ManifestAccessReports {
    entries: Vec<ManifestAccessReport>,
}

impl ManifestAccessReports {
    fn new(entries: Vec<ManifestAccessReport>) -> Self {
        Self { entries }
    }

    fn cleanup_for_paths(
        manifest_path: &Path,
        candidates: &[PathBuf],
        removes_candidates: bool,
        worker: &'static str,
    ) -> Result<Self> {
        let mut entries = Vec::with_capacity(candidates.len() + 1);
        entries.push(ManifestAccessReport::new_checked(
            manifest_path.to_path_buf(),
            AccessIntent::Read,
            worker,
            || Ok(()),
        )?);
        for candidate in candidates {
            entries.push(Self::cleanup_candidate_entry(
                manifest_path,
                candidate,
                removes_candidates,
            )?);
        }
        Ok(Self::new(entries))
    }

    fn cleanup_candidate_entry(
        manifest_path: &Path,
        candidate: &Path,
        removes_candidates: bool,
    ) -> Result<ManifestAccessReport> {
        Self::cleanup_candidate_entry_checked(manifest_path, candidate, removes_candidates, || {
            Ok(())
        })
    }

    fn cleanup_candidate_entry_checked(
        manifest_path: &Path,
        candidate: &Path,
        removes_candidates: bool,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<ManifestAccessReport> {
        check_control()?;
        let path = resolve_manifest_path(manifest_path, candidate);
        let (path, intent) = if removes_candidates {
            (
                checked_write_probe_path(
                    &path,
                    "content manifest cleanup candidate",
                    &mut check_control,
                )?,
                AccessIntent::Write,
            )
        } else {
            (
                existing_read_probe_path(&path)?.to_path_buf(),
                AccessIntent::Read,
            )
        };
        check_control()?;
        ManifestAccessReport::new_checked(
            path,
            intent,
            "content manifest cleanup candidate",
            &mut check_control,
        )
    }

    fn write_for_paths(
        manifest_path: &Path,
        archives: &[ContentArchiveManifestEntry],
    ) -> Result<Self> {
        let mut entries = Vec::with_capacity(archives.len() + 1);
        entries.push(ManifestAccessReport::new_checked(
            checked_write_probe_path(manifest_path, "content manifest write", || Ok(()))?,
            AccessIntent::Write,
            "content manifest write",
            || Ok(()),
        )?);
        for archive in archives {
            entries.push(ManifestAccessReport::new_checked(
                resolve_manifest_path(manifest_path, &archive.path),
                AccessIntent::Read,
                "content manifest write archive",
                || Ok(()),
            )?);
        }
        Ok(Self::new(entries))
    }

    fn recovery_plan_for_paths(
        manifest_path: &Path,
        discovered: &[ContentArchiveManifestEntry],
    ) -> Result<Self> {
        let mut entries = Vec::with_capacity(discovered.len() + 1);
        entries.push(ManifestAccessReport::new_checked(
            existing_read_probe_path(manifest_path)?.to_path_buf(),
            AccessIntent::Read,
            "content manifest recovery plan",
            || Ok(()),
        )?);
        for entry in discovered {
            entries.push(ManifestAccessReport::new_checked(
                existing_read_probe_path(&resolve_manifest_path(manifest_path, &entry.path))?
                    .to_path_buf(),
                AccessIntent::Read,
                "content manifest recovery discovered archive",
                || Ok(()),
            )?);
        }
        Ok(Self::new(entries))
    }

    fn recovery_write_for_paths(manifest_path: &Path, quarantine: &Path) -> Result<Self> {
        let mut entries = Vec::with_capacity(2);
        entries.push(ManifestAccessReport::new_checked(
            checked_write_probe_path(manifest_path, "content manifest recovery manifest", || {
                Ok(())
            })?,
            AccessIntent::Write,
            "content manifest recovery manifest",
            || Ok(()),
        )?);
        entries.push(ManifestAccessReport::new_checked(
            checked_write_probe_path(
                quarantine,
                "content manifest recovery quarantine",
                || Ok(()),
            )?,
            AccessIntent::Write,
            "content manifest recovery quarantine",
            || Ok(()),
        )?);
        Ok(Self::new(entries))
    }

    fn promotion_for_paths(
        manifest_path: &Path,
        new_archive: &ContentArchiveManifestEntry,
        retired_paths: &[PathBuf],
    ) -> Result<Self> {
        let mut entries = Vec::with_capacity(retired_paths.len() + 3);
        entries.push(ManifestAccessReport::new_checked(
            manifest_path.to_path_buf(),
            AccessIntent::Read,
            "content manifest promotion manifest",
            || Ok(()),
        )?);
        entries.push(ManifestAccessReport::new_checked(
            checked_write_probe_path(manifest_path, "content manifest promotion manifest", || {
                Ok(())
            })?,
            AccessIntent::Write,
            "content manifest promotion manifest",
            || Ok(()),
        )?);
        entries.push(ManifestAccessReport::new_checked(
            resolve_manifest_path(manifest_path, &new_archive.path),
            AccessIntent::Read,
            "content manifest promotion archive",
            || Ok(()),
        )?);
        for path in retired_paths {
            entries.push(ManifestAccessReport::new_checked(
                existing_read_probe_path(&resolve_manifest_path(manifest_path, path))?
                    .to_path_buf(),
                AccessIntent::Read,
                "content manifest promotion retirement",
                || Ok(()),
            )?);
        }
        Ok(Self::new(entries))
    }

    fn promotion_recovery_plan_for_path(manifest_path: &Path) -> Result<Self> {
        let journal_path = content_manifest_promotion_journal_path(manifest_path);
        Ok(Self::new(vec![
            ManifestAccessReport::new_checked(
                manifest_path.to_path_buf(),
                AccessIntent::Read,
                "content manifest promotion recovery plan",
                || Ok(()),
            )?,
            ManifestAccessReport::new_checked(
                existing_read_probe_path(&journal_path)?.to_path_buf(),
                AccessIntent::Read,
                "content manifest promotion recovery journal",
                || Ok(()),
            )?,
        ]))
    }

    fn promotion_recovery_for_path(manifest_path: &Path) -> Result<Self> {
        let journal_path = content_manifest_promotion_journal_path(manifest_path);
        Ok(Self::new(vec![
            ManifestAccessReport::new_checked(
                manifest_path.to_path_buf(),
                AccessIntent::Read,
                "content manifest promotion recovery",
                || Ok(()),
            )?,
            ManifestAccessReport::new_checked(
                checked_write_probe_path(
                    manifest_path,
                    "content manifest promotion recovery",
                    || Ok(()),
                )?,
                AccessIntent::Write,
                "content manifest promotion recovery",
                || Ok(()),
            )?,
            ManifestAccessReport::new_checked(
                existing_read_probe_path(&journal_path)?.to_path_buf(),
                AccessIntent::Read,
                "content manifest promotion recovery journal",
                || Ok(()),
            )?,
            ManifestAccessReport::new_checked(
                checked_write_probe_path(
                    &journal_path,
                    "content manifest promotion recovery journal",
                    || Ok(()),
                )?,
                AccessIntent::Write,
                "content manifest promotion recovery journal",
                || Ok(()),
            )?,
        ]))
    }

    fn read_paths_checked(
        paths: &[PathBuf],
        worker: &'static str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let mut entries = Vec::with_capacity(paths.len());
        for path in paths {
            check_control()?;
            entries.push(ManifestAccessReport::new_checked(
                path.clone(),
                AccessIntent::Read,
                worker,
                &mut check_control,
            )?);
        }
        check_control()?;
        Ok(Self::new(entries))
    }

    fn preflight_volumes(&self) -> Result<()> {
        for entry in &self.entries {
            entry.preflight_volume()?;
        }
        Ok(())
    }

    fn access_checked(
        &self,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ScopedAccessGuard>> {
        let mut guards = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            check_control()?;
            guards.push(entry.access_checked(&mut check_control)?);
        }
        check_control()?;
        Ok(guards)
    }

    fn first_volume(&self) -> Option<VolumeId> {
        self.entries.iter().find_map(ManifestAccessReport::volume)
    }
}

#[cfg(test)]
fn retain_manifest_write_access_checked(
    manifest_path: &Path,
    archives: &[ContentArchiveManifestEntry],
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let mut guards = vec![preflight_access_scope_checked(
        &checked_write_probe_path(manifest_path, "content manifest write", &mut check_control)?,
        AccessIntent::Write,
        "content manifest write",
        &mut check_control,
    )?];
    for archive in archives {
        check_control()?;
        guards.push(preflight_access_scope_checked(
            &resolve_manifest_path(manifest_path, &archive.path),
            AccessIntent::Read,
            "content manifest write archive",
            &mut check_control,
        )?);
    }
    check_control()?;
    Ok(guards)
}

fn run_manifest_write(
    manifest_path: PathBuf,
    archives: Vec<ContentArchiveManifestEntry>,
) -> Result<String> {
    const WORKER: &str = "content manifest write";
    let access_reports = ManifestAccessReports::write_for_paths(&manifest_path, &archives)?;
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        let manifest = ContentArchiveManifest::new(archives)?;
        manifest.write_checked(&manifest_path, || cancellation.check())?;
        Ok(format!(
            "content-manifest\tarchives={}",
            manifest.archives.len()
        ))
    })
}

fn run_manifest_inspect(manifest_path: PathBuf) -> Result<Vec<String>> {
    let access_report = ManifestAccessReport::new_checked(
        manifest_path.clone(),
        AccessIntent::Read,
        "content manifest inspect",
        || Ok(()),
    )?;
    access_report.preflight_volume()?;
    let volume = access_report.volume();
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "content manifest inspect",
        move |cancellation| {
            cancellation.check()?;
            let _manifest_access = access_report.access_checked(|| cancellation.check())?;
            let manifest =
                ContentArchiveManifest::read_checked(&manifest_path, || cancellation.check())?;
            let paths = manifest.resolved_archive_paths(&manifest_path);
            let archive_access_reports = ManifestAccessReports::read_paths_checked(
                &paths,
                "content manifest inspect archive",
                || cancellation.check(),
            )?;
            archive_access_reports.preflight_volumes()?;
            cancellation.check()?;
            let _archive_access = archive_access_reports.access_checked(|| cancellation.check())?;
            cancellation.check()?;
            let set = MmapContentSet::open_checked(&paths, || cancellation.check())?;
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
                    escape_manifest_tsv_path(&entry.path),
                    escape_manifest_tsv_path(&path)
                ));
            }
            Ok(lines)
        },
    )
}

fn run_manifest_recovery_plan(
    manifest_path: PathBuf,
    discovered: Vec<ContentArchiveManifestEntry>,
) -> Result<Vec<String>> {
    let access_reports =
        ManifestAccessReports::recovery_plan_for_paths(&manifest_path, &discovered)?;
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "content manifest recovery plan",
        move |cancellation| {
            cancellation.check()?;
            let _access = access_reports.access_checked(|| cancellation.check())?;
            cancellation.check()?;
            let plan = plan_content_manifest_recovery_checked(&manifest_path, &discovered, || {
                cancellation.check()
            })?;
            cancellation.check()?;
            let mut lines = vec![plan.as_tsv()];
            lines.extend(format_content_archive_health(
                "invalid",
                &plan.invalid_archives,
            ));
            Ok(lines)
        },
    )
}

fn run_manifest_recover(
    manifest_path: PathBuf,
    quarantine: PathBuf,
    discovered: Vec<ContentArchiveManifestEntry>,
) -> Result<Vec<String>> {
    let mut access_reports =
        ManifestAccessReports::recovery_plan_for_paths(&manifest_path, &discovered)?;
    access_reports.preflight_volumes()?;
    let write_reports =
        ManifestAccessReports::recovery_write_for_paths(&manifest_path, &quarantine)?;
    write_reports.preflight_volumes()?;
    access_reports.entries.extend(write_reports.entries);
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "content manifest recovery",
        move |cancellation| {
            cancellation.check()?;
            let _access = access_reports.access_checked(|| cancellation.check())?;
            cancellation.check()?;
            let report =
                recover_content_manifest_checked(&manifest_path, &discovered, &quarantine, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            let mut lines = vec![
                report.before.as_tsv(),
                format!(
                    "content-manifest-recovery\twrote-manifest={}\tquarantined-manifest={}",
                    report.wrote_manifest,
                    report
                        .quarantined_manifest_path
                        .as_ref()
                        .map(|path| escape_manifest_tsv_path(path))
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

fn run_manifest_promotion(
    manifest_path: PathBuf,
    new_archive: ContentArchiveManifestEntry,
    retired_paths: Vec<PathBuf>,
) -> Result<(String, Vec<String>)> {
    const WORKER: &str = "content manifest promotion";
    let access_reports =
        ManifestAccessReports::promotion_for_paths(&manifest_path, &new_archive, &retired_paths)?;
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        let promotion = promote_content_archive_manifest_checked(
            &manifest_path,
            new_archive,
            &retired_paths,
            || cancellation.check(),
        )?;
        let summary = format!(
            "content-manifest-promoted\tarchives={}\tretired={}\tmissing-retirements={}",
            promotion.manifest.archives.len(),
            promotion.retired_archives.len(),
            promotion.missing_retirements.len()
        );
        let mut lines = Vec::new();
        for path in promotion.retired_archives {
            lines.push(format!("retire\t{}", escape_manifest_tsv_path(&path)));
        }
        for path in promotion.missing_retirements {
            lines.push(format!(
                "missing-retirement\t{}",
                escape_manifest_tsv_path(&path)
            ));
        }
        Ok((summary, lines))
    })
}

fn run_manifest_promotion_recovery_plan(manifest_path: PathBuf) -> Result<String> {
    const WORKER: &str = "content manifest promotion recovery plan";
    const ARCHIVE_WORKER: &str = "content manifest promotion recovery archive";
    let access_reports = ManifestAccessReports::promotion_recovery_plan_for_path(&manifest_path)?;
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(|| cancellation.check())?;
        let journal_path = content_manifest_promotion_journal_path(&manifest_path);
        let archive_paths = if manifest_path_exists(&journal_path, "promotion journal")? {
            let journal = ContentManifestPromotionJournal::read_checked(&journal_path, || {
                cancellation.check()
            })?;
            promotion_recovery_archive_paths(&manifest_path, &journal)?
        } else {
            Vec::new()
        };
        let archive_access_reports =
            ManifestAccessReports::read_paths_checked(&archive_paths, ARCHIVE_WORKER, || {
                cancellation.check()
            })?;
        archive_access_reports.preflight_volumes()?;
        cancellation.check()?;
        let _archive_access = archive_access_reports.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        Ok(
            plan_content_manifest_promotion_recovery_checked(manifest_path, || {
                cancellation.check()
            })?
            .as_tsv(),
        )
    })
}

fn run_manifest_promotion_recover(manifest_path: PathBuf) -> Result<Vec<String>> {
    const WORKER: &str = "content manifest promotion recovery";
    const ARCHIVE_WORKER: &str = "content manifest promotion recovery archive";
    let access_reports = ManifestAccessReports::promotion_recovery_for_path(&manifest_path)?;
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(|| cancellation.check())?;
        let journal_path = content_manifest_promotion_journal_path(&manifest_path);
        let archive_paths = if manifest_path_exists(&journal_path, "promotion journal")? {
            let journal = ContentManifestPromotionJournal::read_checked(&journal_path, || {
                cancellation.check()
            })?;
            promotion_recovery_archive_paths(&manifest_path, &journal)?
        } else {
            Vec::new()
        };
        let archive_access_reports =
            ManifestAccessReports::read_paths_checked(&archive_paths, ARCHIVE_WORKER, || {
                cancellation.check()
            })?;
        archive_access_reports.preflight_volumes()?;
        cancellation.check()?;
        let _archive_access = archive_access_reports.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        let recovery =
            recover_content_manifest_promotion_checked(manifest_path, || cancellation.check())?;
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

#[cfg(test)]
fn retain_manifest_cleanup_access_checked(
    manifest_path: &Path,
    candidates: &[PathBuf],
    removes_candidates: bool,
    worker: &'static str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let mut guards = vec![ManifestAccessReport::new_checked(
        manifest_path.to_path_buf(),
        AccessIntent::Read,
        worker,
        &mut check_control,
    )?
    .access_checked(&mut check_control)?];
    for candidate in candidates {
        check_control()?;
        let report = ManifestAccessReports::cleanup_candidate_entry_checked(
            manifest_path,
            candidate,
            removes_candidates,
            &mut check_control,
        )?;
        check_control()?;
        guards.push(report.access_checked(&mut check_control)?);
    }
    check_control()?;
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

fn write_probe_path(path: &Path) -> Result<&Path> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Ok(crate::parent_or_cwd(path)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("manifest write path metadata unavailable: {err}"),
        )),
    }
}

fn checked_write_probe_path(
    path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<PathBuf> {
    check_control()?;
    preflight_write_target_volume_checked(path, worker, &mut check_control)?;
    check_control()?;
    let probe = write_probe_path(path)?.to_path_buf();
    check_control()?;
    Ok(probe)
}

fn preflight_write_target_volume_checked(
    path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let volume_path = crate::parent_or_cwd(path);
    let volume_report =
        VolumeDiscoveryReport::for_containing_path_policy_checked(volume_path, &mut check_control)?;
    check_control()?;
    preflight_volume_access_scope_with_report(
        volume_path,
        AccessIntent::Write,
        worker,
        &volume_report,
    )
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
    write_probe_path(path)
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
                escape_manifest_tsv_path(&archive.resolved_path),
                archive
                    .detail
                    .as_deref()
                    .map(escape_manifest_tsv_field)
                    .unwrap_or_else(|| "-".to_string())
            )
        })
        .collect()
}

fn escape_manifest_tsv_path(path: &Path) -> String {
    escape_manifest_tsv_field(&path.to_string_lossy())
}

fn escape_manifest_tsv_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn manifest_write_access_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-manifest-write-access-cancelled");
        let manifest_path = root.join("content-manifest.tsv");
        fs::write(&manifest_path, b"old manifest").expect("write manifest");
        let archives = vec![manifest_entry(root.join("content.gfmcontent"))];

        let result = retain_manifest_write_access_checked(&manifest_path, &archives, || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_write_access_refuses_unreachable_manifest_before_write_probe() {
        let root = unique_temp_dir("gfm-manifest-write-access-unreachable-root");
        let offline = unique_temp_dir("gfm-manifest-write-access-unreachable-output");
        fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let manifest_path = offline.join("manifest-unavailable".repeat(16));
        let archives = vec![manifest_entry(root.join("content.gfmcontent"))];

        let err = match retain_manifest_write_access_checked(&manifest_path, &archives, || Ok(())) {
            Ok(_) => panic!("unreachable manifest was admitted before volume preflight"),
            Err(err) => err,
        };

        assert!(
            err.to_string().contains(
                "content manifest write volume access blocked: unreachable volume network"
            ),
            "{err}"
        );
        assert!(
            !err.to_string()
                .contains("manifest write path metadata unavailable"),
            "{err}"
        );
        assert!(!manifest_path.exists());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(offline);
    }

    #[test]
    fn manifest_access_report_checked_honors_pre_cancelled_control_before_volume_discovery() {
        let path = std::env::temp_dir()
            .join(format!(
                "gfm-manifest-report-pre-cancel-{}",
                std::process::id()
            ))
            .join("content-manifest.tsv");

        let result = ManifestAccessReport::new_checked(
            path.clone(),
            AccessIntent::Read,
            "content manifest inspect",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn manifest_read_paths_checked_can_cancel_between_paths() {
        let root = unique_temp_dir("gfm-manifest-read-report-cancelled");
        let first = root.join("first.gfmcontent");
        let second = root.join("second.gfmcontent");
        fs::write(&first, b"first").expect("write first archive");
        fs::write(&second, b"second").expect("write second archive");
        let calls = AtomicUsize::new(0);

        let result = ManifestAccessReports::read_paths_checked(
            &[first, second],
            "content manifest inspect archive",
            || {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                if call >= 4 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(calls.load(Ordering::SeqCst) > 4);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_cleanup_access_checked_rechecks_control_during_candidate_walk() {
        let root = unique_temp_dir("gfm-manifest-cleanup-access-cancelled");
        let manifest_path = root.join("content-manifest.tsv");
        let first = root.join("first.gfmcontent");
        let second = root.join("second.gfmcontent");
        fs::write(&manifest_path, b"manifest").expect("write manifest");
        fs::write(&first, b"first").expect("write first archive");
        fs::write(&second, b"second").expect("write second archive");
        let calls = AtomicUsize::new(0);

        let result = retain_manifest_cleanup_access_checked(
            &manifest_path,
            &[first, second],
            false,
            "content manifest cleanup",
            || {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                if call >= 4 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(calls.load(Ordering::SeqCst) > 4);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_cleanup_access_refuses_unreachable_candidate_before_write_probe() {
        let root = unique_temp_dir("gfm-manifest-cleanup-access-root");
        let offline = unique_temp_dir("gfm-manifest-cleanup-access-offline");
        fs::write(offline.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let manifest_path = root.join("content-manifest.tsv");
        fs::write(&manifest_path, b"manifest").expect("write manifest");
        let candidate = offline.join("cleanup-unavailable".repeat(16));

        let err = match retain_manifest_cleanup_access_checked(
            &manifest_path,
            std::slice::from_ref(&candidate),
            true,
            "content manifest cleanup",
            || Ok(()),
        ) {
            Ok(_) => panic!("unreachable cleanup candidate was admitted before volume preflight"),
            Err(err) => err,
        };

        assert!(
            err.to_string().contains(
                "content manifest cleanup candidate volume access blocked: unreachable volume network"
            ),
            "{err}"
        );
        assert!(
            !err.to_string()
                .contains("manifest write path metadata unavailable"),
            "{err}"
        );
        assert!(!candidate.exists());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(offline);
    }

    #[test]
    fn manifest_promotion_recovery_archive_access_checked_can_cancel_before_archive_probe() {
        let root = unique_temp_dir("gfm-manifest-promotion-archive-access-cancelled");
        let archive = root.join("active.gfmcontent");
        fs::write(&archive, b"archive").expect("write active archive");
        let access_reports = ManifestAccessReports::read_paths_checked(
            &[archive],
            "content manifest promotion recovery archive",
            || Ok(()),
        )
        .unwrap();

        let result = access_reports.access_checked(|| Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_tsv_helpers_escape_path_fields() {
        let path = PathBuf::from("/tmp/Manifest\\Rows/Archive\tDraft\nFinal\r.gfmcontent");

        assert_eq!(
            escape_manifest_tsv_path(&path),
            "/tmp/Manifest\\\\Rows/Archive\\tDraft\\nFinal\\r.gfmcontent"
        );
        assert_eq!(
            escape_manifest_tsv_field("detail\tbad\nrow\r\\"),
            "detail\\tbad\\nrow\\r\\\\"
        );
    }

    fn manifest_entry(path: PathBuf) -> ContentArchiveManifestEntry {
        ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path,
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp directory");
        path
    }
}
