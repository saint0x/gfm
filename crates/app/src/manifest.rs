use crate::{parse_u64_arg, parse_usize_arg, required_path};
use gfm_index::{
    ContentArchiveCleanupPolicy, ContentArchiveManifest, ContentArchiveManifestEntry,
    ContentMergeTier,
};
use gfm_store::{
    plan_content_manifest_promotion_recovery, plan_content_manifest_recovery,
    promote_content_archive_manifest, recover_content_manifest, recover_content_manifest_promotion,
    ContentArchiveHealth, MmapContentSet,
};
use gfm_types::{GfmError, Result};
use std::path::PathBuf;

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
            let manifest = ContentArchiveManifest::new(archives)?;
            manifest.write(&output)?;
            eprintln!("content-manifest\tarchives={}", manifest.archives.len());
        }
        "content-manifest-inspect" => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-inspect requires a manifest path",
            )?;
            let manifest = ContentArchiveManifest::read(&manifest_path)?;
            let paths = manifest.resolved_archive_paths(&manifest_path);
            let set = MmapContentSet::open(&paths)?;
            println!(
                "content-manifest\tarchives={}\tterms={}\tbytes={}",
                set.archive_count(),
                set.indexed_terms(),
                set.mapped_len()
            );
            for (entry, path) in manifest.archives.iter().zip(paths) {
                println!(
                    "archive\t{}\t{}\t{}",
                    content_tier_name(entry.tier),
                    entry.path.display(),
                    path.display()
                );
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
            let plan = plan_content_manifest_recovery(&manifest_path, &discovered);
            println!("{}", plan.as_tsv());
            print_content_archive_health("invalid", &plan.invalid_archives);
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
            let report = recover_content_manifest(&manifest_path, &discovered, &quarantine)?;
            println!("{}", report.before.as_tsv());
            println!(
                "content-manifest-recovery\twrote-manifest={}\tquarantined-manifest={}",
                report.wrote_manifest,
                report
                    .quarantined_manifest_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
            println!("{}", report.after.as_tsv());
            print_content_archive_health("invalid-before", &report.before.invalid_archives);
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
            let promotion =
                promote_content_archive_manifest(&manifest_path, new_archive, &retired_paths)?;
            eprintln!(
                "content-manifest-promoted\tarchives={}\tretired={}\tmissing-retirements={}",
                promotion.manifest.archives.len(),
                promotion.retired_archives.len(),
                promotion.missing_retirements.len()
            );
            for path in promotion.retired_archives {
                println!("retire\t{}", path.display());
            }
            for path in promotion.missing_retirements {
                println!("missing-retirement\t{}", path.display());
            }
        }
        "content-manifest-promotion-recovery-plan" => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-promotion-recovery-plan requires a manifest path",
            )?;
            println!(
                "{}",
                plan_content_manifest_promotion_recovery(manifest_path).as_tsv()
            );
        }
        "content-manifest-promotion-recover" => {
            let manifest_path = required_path(
                args.next(),
                "content-manifest-promotion-recover requires a manifest path",
            )?;
            let recovery = recover_content_manifest_promotion(manifest_path)?;
            println!("{}", recovery.before.as_tsv());
            println!(
                "content-manifest-promotion-recovery\tcompleted-promotion={}\tremoved-journal={}",
                recovery.completed_promotion, recovery.removed_journal
            );
            println!("{}", recovery.after.as_tsv());
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
            let manifest = ContentArchiveManifest::read(&manifest_path)?;
            let report = manifest.cleanup_inactive_archives(&manifest_path, &candidates)?;
            eprintln!(
                "content-manifest-cleanup\tremoved={}\tactive={}\tmissing={}",
                report.removed_archives.len(),
                report.active_archives.len(),
                report.missing_archives.len()
            );
            for path in report.removed_archives {
                println!("removed\t{}", path.display());
            }
            for path in report.active_archives {
                println!("active\t{}", path.display());
            }
            for path in report.missing_archives {
                println!("missing\t{}", path.display());
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
            let manifest = ContentArchiveManifest::read(&manifest_path)?;
            let plan = manifest.plan_inactive_archive_cleanup(
                &manifest_path,
                &candidates,
                &ContentArchiveCleanupPolicy {
                    min_retired_archives,
                    min_retired_bytes,
                    max_cleanup_archives,
                },
            )?;
            eprintln!(
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
            for path in plan.cleanup_archives {
                println!("cleanup\t{}", path.display());
            }
            for path in plan.deferred_archives {
                println!("defer\t{}", path.display());
            }
            for path in plan.active_archives {
                println!("active\t{}", path.display());
            }
            for path in plan.missing_archives {
                println!("missing\t{}", path.display());
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
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

fn print_content_archive_health(label: &str, archives: &[ContentArchiveHealth]) {
    for archive in archives {
        println!(
            "{}\t{}\t{}\t{}",
            label,
            content_tier_name(archive.entry.tier),
            archive.resolved_path.display(),
            archive.detail.as_deref().unwrap_or("-")
        );
    }
}
