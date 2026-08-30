use crate::access::{
    preflight_access_scope_checked, preflight_volume_access_scope, ScopedAccessGuard,
};
use crate::runtime::{
    run_retriable_volume_task_cancellable_with_payload_path,
    run_scheduled_volume_task_cancellable_with_volume_and_payload_path,
    run_volume_task_cancellable,
};
use crate::{
    detect_volume_id, optional_path_arg, parent_volume, parse_required_scheduling_pressure,
    parse_u64_arg, required_path,
};
use gfm_index::{ContentArchiveManifestEntry, ContentMergeTier};
use gfm_jobs::{Cancellation, Priority};
use gfm_mac::AccessIntent;
use gfm_store::{
    atomic_write_checked, dictionary_term_report_from_records, fuzzy_postings_from_records,
    inspect_archive_schema_checked, metadata_postings_from_records,
    migrate_content_archive_checked, migrate_metadata_archive_checked,
    migrate_record_archive_checked, plan_archive_rebuilds, plan_columns_archive_rebuild,
    plan_content_archive_migration_checked, plan_derived_sidecar_rebuild,
    plan_metadata_archive_migration_checked, plan_record_archive_migration_checked,
    plan_sidecar_recovery_checked, prefix_postings_from_records, rebuild_columns_archive,
    rebuild_derived_sidecar_checked, recover_sidecars_checked, sidecar_kind_name,
    substring_postings_from_records, write_dictionary_checked, write_fuzzy_postings_checked,
    write_metadata_postings_checked, write_prefix_postings_checked, write_record_columns_checked,
    write_substring_postings_checked, ArchiveRebuildInputs, ArchiveSchemaKind,
    ColumnsArchiveRebuild, MmapRecordArchive, MmapRecordColumns, SidecarHealth, SidecarKind,
    SidecarPaths, SidecarRecovery, SidecarRecoveryPlan,
};
use gfm_types::{FileId, FileRecord, GfmError, Result, VolumeId};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "records-verify" => {
            let records = required_path(args.next(), "records-verify requires a records path")?;
            let report = run_archive_read_cancellable(
                records,
                "records verify",
                move |records, cancellation| {
                    let archive =
                        MmapRecordArchive::open_checked(records, || cancellation.check())?;
                    cancellation.check()?;
                    let report = format!(
                        "records-verify\trecords={}\tbytes={}\tchecksum={}",
                        archive.len(),
                        archive.mapped_len(),
                        if archive.is_checksummed() {
                            "verified"
                        } else {
                            "legacy"
                        }
                    );
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
            println!("{report}");
        }
        "archive-schema" => {
            let kind = args
                .next()
                .and_then(|kind| ArchiveSchemaKind::parse(&kind))
                .ok_or_else(|| {
                    GfmError::Format(
                        "archive-schema requires records, columns, metadata, prefixes, substrings, fuzzy, dictionary, content, or content-manifest".to_string(),
                    )
            })?;
            let path = required_path(args.next(), "archive-schema requires an archive path")?;
            let report =
                run_archive_read_cancellable(path, "archive schema", move |path, cancellation| {
                    cancellation.check()?;
                    let report =
                        inspect_archive_schema_checked(kind, path, || cancellation.check())?
                            .as_tsv();
                    cancellation.check()?;
                    Ok(report)
                })?;
            println!("{report}");
        }
        "archive-rebuild-plan" => {
            let records =
                required_path(args.next(), "archive-rebuild-plan requires a records path")?;
            let columns =
                required_path(args.next(), "archive-rebuild-plan requires a columns path")?;
            let metadata =
                required_path(args.next(), "archive-rebuild-plan requires a metadata path")?;
            let prefixes =
                required_path(args.next(), "archive-rebuild-plan requires a prefixes path")?;
            let substrings = required_path(
                args.next(),
                "archive-rebuild-plan requires a substrings path",
            )?;
            let fuzzy = required_path(args.next(), "archive-rebuild-plan requires a fuzzy path")?;
            let dictionary = required_path(
                args.next(),
                "archive-rebuild-plan requires a dictionary path",
            )?;
            let content =
                required_path(args.next(), "archive-rebuild-plan requires a content path")?;
            let manifest = required_path(
                args.next(),
                "archive-rebuild-plan requires a content manifest path",
            )?;
            let discovered_archives = args
                .map(|spec| parse_content_manifest_archive_spec(&spec))
                .collect::<Result<Vec<_>>>()?;
            let inputs = ArchiveRebuildInputs {
                records_path: records,
                columns_path: columns,
                metadata_path: metadata,
                prefixes_path: prefixes,
                substrings_path: substrings,
                fuzzy_path: fuzzy,
                dictionary_path: dictionary,
                content_path: content,
                manifest_path: manifest,
                discovered_content_archives: discovered_archives,
            };
            let lines = run_archive_rebuild_plan(inputs)?;
            for line in lines {
                println!("{line}");
            }
        }
        "records-migration-plan" => {
            let records = required_path(
                args.next(),
                "records-migration-plan requires a records path",
            )?;
            let report = run_archive_read_cancellable(
                records,
                "records migration plan",
                move |records, cancellation| {
                    cancellation.check()?;
                    let report =
                        plan_record_archive_migration_checked(records, || cancellation.check())?
                            .as_tsv();
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
            println!("{report}");
        }
        "records-migrate" => {
            let records = required_path(args.next(), "records-migrate requires a records path")?;
            let backup_dir =
                required_path(args.next(), "records-migrate requires a backup directory")?;
            let migration = run_archive_migration(
                records,
                backup_dir,
                "records migrate",
                |archive, backup_dir, cancellation| {
                    migrate_record_archive_checked(archive, backup_dir, || cancellation.check())
                },
            )?;
            println!("{}", migration.as_tsv());
        }
        "content-migration-plan" => {
            let content = required_path(
                args.next(),
                "content-migration-plan requires a content path",
            )?;
            let report = run_archive_read_cancellable(
                content,
                "content migration plan",
                move |content, cancellation| {
                    cancellation.check()?;
                    let report =
                        plan_content_archive_migration_checked(content, || cancellation.check())?
                            .as_tsv();
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
            println!("{report}");
        }
        "content-migrate" => {
            let content = required_path(args.next(), "content-migrate requires a content path")?;
            let backup_dir =
                required_path(args.next(), "content-migrate requires a backup directory")?;
            let migration = run_archive_migration(
                content,
                backup_dir,
                "content migrate",
                |archive, backup_dir, cancellation| {
                    migrate_content_archive_checked(archive, backup_dir, || cancellation.check())
                },
            )?;
            println!("{}", migration.as_tsv());
        }
        "metadata-migration-plan" => {
            let metadata = required_path(
                args.next(),
                "metadata-migration-plan requires a metadata path",
            )?;
            let report = run_archive_read_cancellable(
                metadata,
                "metadata migration plan",
                move |metadata, cancellation| {
                    cancellation.check()?;
                    let report =
                        plan_metadata_archive_migration_checked(metadata, || cancellation.check())?
                            .as_tsv();
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
            println!("{report}");
        }
        "metadata-migrate" => {
            let metadata = required_path(args.next(), "metadata-migrate requires a metadata path")?;
            let backup_dir =
                required_path(args.next(), "metadata-migrate requires a backup directory")?;
            let migration = run_archive_migration(
                metadata,
                backup_dir,
                "metadata migrate",
                |archive, backup_dir, cancellation| {
                    migrate_metadata_archive_checked(archive, backup_dir, || cancellation.check())
                },
            )?;
            println!("{}", migration.as_tsv());
        }
        "columns-rebuild-plan" => {
            let records =
                required_path(args.next(), "columns-rebuild-plan requires a records path")?;
            let columns =
                required_path(args.next(), "columns-rebuild-plan requires a columns path")?;
            let report = run_columns_rebuild_plan(records, columns)?;
            println!("{report}");
        }
        "columns-rebuild" => {
            let records = required_path(args.next(), "columns-rebuild requires a records path")?;
            let columns = required_path(args.next(), "columns-rebuild requires a columns path")?;
            let backup_dir =
                required_path(args.next(), "columns-rebuild requires a backup directory")?;
            let rebuild = run_columns_rebuild(records, columns, backup_dir)?;
            println!("{}", rebuild.as_tsv());
        }
        "derived-sidecar-rebuild-plan" => {
            let records = required_path(
                args.next(),
                "derived-sidecar-rebuild-plan requires a records path",
            )?;
            let kind = parse_sidecar_kind(args.next(), "derived-sidecar-rebuild-plan")?;
            let sidecar = required_path(
                args.next(),
                "derived-sidecar-rebuild-plan requires a sidecar path",
            )?;
            let report = run_derived_sidecar_rebuild_plan(records, kind, sidecar)?;
            println!("{report}");
        }
        "derived-sidecar-rebuild" | "derived-sidecar-rebuild-retry-probe" => {
            let records = required_path(
                args.next(),
                "derived-sidecar-rebuild requires a records path",
            )?;
            let kind = parse_sidecar_kind(args.next(), "derived-sidecar-rebuild")?;
            let sidecar = required_path(
                args.next(),
                "derived-sidecar-rebuild requires a sidecar path",
            )?;
            let backup_dir = required_path(
                args.next(),
                "derived-sidecar-rebuild requires a backup directory",
            )?;
            let retry_probe = if command == "derived-sidecar-rebuild-retry-probe" {
                Some(required_path(
                    args.next(),
                    "derived-sidecar-rebuild-retry-probe requires an attempt state path",
                )?)
            } else {
                None
            };
            let volume = detect_volume_id(&records)
                .ok()
                .or_else(|| parent_volume(&records));
            preflight_derived_sidecar_rebuild_volumes(&records, &sidecar, &backup_dir)?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                preflight_volume_access_scope(
                    write_probe_path(retry_probe)?,
                    AccessIntent::Write,
                    "derived sidecar rebuild",
                )?;
            }
            let rebuild = run_retriable_volume_task_cancellable_with_payload_path(
                volume,
                Priority::Visible,
                "derived sidecar rebuild",
                sidecar.clone(),
                move |cancellation| {
                    let records = records.clone();
                    let sidecar = sidecar.clone();
                    let backup_dir = backup_dir.clone();
                    let retry_probe = retry_probe.clone();
                    cancellation.check()?;
                    if let Some(retry_probe) = retry_probe.as_ref() {
                        fail_first_archive_retry_probe_attempt(
                            retry_probe,
                            "derived sidecar rebuild",
                            &cancellation,
                        )?;
                    }
                    let _access = retain_derived_sidecar_rebuild_access_checked(
                        &records,
                        &sidecar,
                        &backup_dir,
                        || cancellation.check(),
                    )?;
                    cancellation.check()?;
                    rebuild_derived_sidecar_checked(records, kind, sidecar, backup_dir, || {
                        cancellation.check()
                    })
                },
            )?;
            println!("{}", rebuild.as_tsv());
        }
        "index-columns" => {
            let records = required_path(args.next(), "index-columns requires a records path")?;
            let output =
                required_path(args.next(), "index-columns requires an output columns path")?;
            let records = build_record_sidecar(
                records,
                output,
                "index columns",
                |output, records, cancellation| {
                    let count = records.len();
                    write_record_columns_checked(output, &records, || cancellation.check())?;
                    Ok(count)
                },
            )?;
            eprintln!("columns-indexed {} records", records);
        }
        "columns-verify" => {
            let columns = required_path(args.next(), "columns-verify requires a columns path")?;
            let report = run_archive_read_cancellable(
                columns,
                "columns verify",
                move |columns, cancellation| {
                    let archive =
                        MmapRecordColumns::open_checked(columns, || cancellation.check())?;
                    cancellation.check()?;
                    let report = format!(
                        "columns-verify\trecords={}\tbytes={}\tchecksum={}",
                        archive.len(),
                        archive.mapped_len(),
                        if archive.is_checksummed() {
                            "verified"
                        } else {
                            "legacy"
                        }
                    );
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
            println!("{report}");
        }
        "columns-lookup" => {
            let columns = required_path(args.next(), "columns-lookup requires a columns path")?;
            let volume = parse_u64_arg(args.next(), "columns-lookup requires a volume id")?;
            let node = parse_u64_arg(args.next(), "columns-lookup requires a node id")?;
            let report = run_archive_read_cancellable(
                columns,
                "columns lookup",
                move |columns, cancellation| {
                    let archive =
                        MmapRecordColumns::open_checked(columns, || cancellation.check())?;
                    cancellation.check()?;
                    let report = match archive.find(FileId::new(VolumeId(volume), node))? {
                        Some(column) => format!(
                        "columns\tfound\tid={}:{}\tname={}\text={}\ttags={}\tcomment={}\tpath={}",
                        column.id.volume.0,
                        column.id.node,
                        column.name,
                        column.extension.as_deref().unwrap_or(""),
                        column.tags.join(","),
                        column.comment.as_deref().unwrap_or(""),
                        column.path
                    ),
                        None => format!("columns\tmissing\tid={volume}:{node}"),
                    };
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
            println!("{report}");
        }
        "index-metadata" => {
            let records = required_path(args.next(), "index-metadata requires a records path")?;
            let output = required_path(
                args.next(),
                "index-metadata requires an output metadata path",
            )?;
            let terms = build_record_sidecar(
                records,
                output,
                "index metadata",
                |output, records, cancellation| {
                    let postings = metadata_postings_from_records(&records);
                    let terms = postings.len();
                    write_metadata_postings_checked(output, &postings, || cancellation.check())?;
                    Ok(terms)
                },
            )?;
            eprintln!("metadata-indexed {} terms", terms);
        }
        "index-dictionary" => {
            let records = required_path(args.next(), "index-dictionary requires a records path")?;
            let output = required_path(
                args.next(),
                "index-dictionary requires an output dictionary path",
            )?;
            let report = build_record_sidecar(
                records,
                output,
                "index dictionary",
                |output, records, cancellation| {
                    let report = dictionary_term_report_from_records(&records);
                    write_dictionary_checked(output, &report.terms, || cancellation.check())?;
                    Ok(report)
                },
            )?;
            eprintln!(
                "dictionary-indexed\tterms={}\tpaths={}\tpath-prefixes={}\textensions={}\ttags={}\tkinds={}\tmetadata-keys={}\tcomment-tokens={}",
                report.terms.len(),
                report.paths,
                report.path_prefixes,
                report.extensions,
                report.tags,
                report.kinds,
                report.metadata_keys,
                report.comment_tokens
            );
        }
        "index-prefixes" => {
            let records = required_path(args.next(), "index-prefixes requires a records path")?;
            let output =
                required_path(args.next(), "index-prefixes requires an output prefix path")?;
            let prefixes = build_record_sidecar(
                records,
                output,
                "index prefixes",
                |output, records, cancellation| {
                    let postings = prefix_postings_from_records(&records);
                    let prefixes = postings.len();
                    write_prefix_postings_checked(output, &postings, || cancellation.check())?;
                    Ok(prefixes)
                },
            )?;
            eprintln!("prefixes-indexed {} prefixes", prefixes);
        }
        "index-substrings" => {
            let records = required_path(args.next(), "index-substrings requires a records path")?;
            let output = required_path(
                args.next(),
                "index-substrings requires an output substring path",
            )?;
            let grams = build_record_sidecar(
                records,
                output,
                "index substrings",
                |output, records, cancellation| {
                    let postings = substring_postings_from_records(&records);
                    let grams = postings.len();
                    write_substring_postings_checked(output, &postings, || cancellation.check())?;
                    Ok(grams)
                },
            )?;
            eprintln!("substrings-indexed {} grams", grams);
        }
        "index-fuzzy" => {
            let records = required_path(args.next(), "index-fuzzy requires a records path")?;
            let output = required_path(args.next(), "index-fuzzy requires an output fuzzy path")?;
            let keys = build_record_sidecar(
                records,
                output,
                "index fuzzy",
                |output, records, cancellation| {
                    let postings = fuzzy_postings_from_records(&records);
                    let keys = postings.len();
                    write_fuzzy_postings_checked(output, &postings, || cancellation.check())?;
                    Ok(keys)
                },
            )?;
            eprintln!("fuzzy-indexed {} keys", keys);
        }
        "sidecar-recovery-plan" => {
            let records =
                required_path(args.next(), "sidecar-recovery-plan requires a records path")?;
            let sidecars = parse_sidecar_paths(args, "sidecar-recovery-plan")?;
            let plan = run_sidecar_recovery_plan(records, sidecars)?;
            println!("{}", plan.as_tsv());
            print_sidecar_health("invalid", &plan.invalid_sidecars);
        }
        "sidecar-recover" | "sidecar-recover-retry-probe" => {
            let records = required_path(args.next(), "sidecar-recover requires a records path")?;
            let quarantine = required_path(
                args.next(),
                "sidecar-recover requires a quarantine directory",
            )?;
            let retry_probe = if command == "sidecar-recover-retry-probe" {
                Some(required_path(
                    args.next(),
                    "sidecar-recover-retry-probe requires an attempt state path",
                )?)
            } else {
                None
            };
            let sidecars = parse_sidecar_paths(args, "sidecar-recover")?;
            let volume = detect_volume_id(&records)
                .ok()
                .or_else(|| parent_volume(&records));
            preflight_sidecar_recovery_volumes(&records, &sidecars, &quarantine)?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                preflight_volume_access_scope(
                    write_probe_path(retry_probe)?,
                    AccessIntent::Write,
                    "sidecar repair",
                )?;
            }
            let report = run_retriable_volume_task_cancellable_with_payload_path(
                volume,
                Priority::Visible,
                "sidecar repair",
                quarantine.clone(),
                move |cancellation| {
                    let records = records.clone();
                    let sidecars = sidecars.clone();
                    let quarantine = quarantine.clone();
                    let retry_probe = retry_probe.clone();
                    cancellation.check()?;
                    if let Some(retry_probe) = retry_probe.as_ref() {
                        fail_first_archive_retry_probe_attempt(
                            retry_probe,
                            "sidecar repair",
                            &cancellation,
                        )?;
                    }
                    let _access = retain_sidecar_recovery_access_checked(
                        &records,
                        &sidecars,
                        &quarantine,
                        || cancellation.check(),
                    )?;
                    recover_sidecars_checked(&records, &sidecars, &quarantine, || {
                        cancellation.check()
                    })
                },
            )?;
            print_sidecar_recovery_report(report);
        }
        "sidecar-recover-adaptive" => {
            let records = required_path(
                args.next(),
                "sidecar-recover-adaptive requires a records path",
            )?;
            let quarantine = required_path(
                args.next(),
                "sidecar-recover-adaptive requires a quarantine directory",
            )?;
            let pressure = parse_required_scheduling_pressure(args, "sidecar repair")?;
            let sidecars = parse_sidecar_paths(args, "sidecar-recover-adaptive")?;
            let volume_records = records.clone();
            let volume_quarantine = quarantine.clone();
            let volume_sidecars = sidecars.clone();
            let outcome = run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
                Priority::Background,
                "sidecar repair",
                pressure,
                move || {
                    preflight_sidecar_recovery_volumes(
                        &volume_records,
                        &volume_sidecars,
                        &volume_quarantine,
                    )?;
                    Ok(detect_volume_id(&volume_records)
                        .ok()
                        .or_else(|| parent_volume(&volume_records)))
                },
                records.clone(),
                move |cancellation| {
                    let _access = retain_sidecar_recovery_access_checked(
                        &records,
                        &sidecars,
                        &quarantine,
                        || cancellation.check(),
                    )?;
                    recover_sidecars_checked(&records, &sidecars, &quarantine, || {
                        cancellation.check()
                    })
                },
            )?;
            if outcome.deferred {
                eprintln!(
                    "sidecar-recovery-deferred\taction={:?}",
                    outcome.scheduling_action
                );
            } else {
                let report = outcome.result.ok_or_else(|| {
                    GfmError::Format("sidecar repair ran without a report".to_string())
                })?;
                eprintln!("sidecar-recovery-action\t{:?}", outcome.scheduling_action);
                print_sidecar_recovery_report(report);
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn retain_archive_read_access_checked(
    path: &Path,
    worker: &str,
    check_control: impl FnMut() -> Result<()>,
) -> Result<ScopedAccessGuard> {
    preflight_access_scope_checked(path, AccessIntent::Read, worker, check_control)
}

fn run_archive_read_cancellable<T>(
    path: PathBuf,
    worker: &'static str,
    read: impl FnOnce(PathBuf, &Cancellation) -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    preflight_volume_access_scope(&path, AccessIntent::Read, worker)?;
    let volume = detect_volume_id(&path)
        .ok()
        .or_else(|| parent_volume(&path));
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access = retain_archive_read_access_checked(&path, worker, || cancellation.check())?;
        cancellation.check()?;
        read(path, &cancellation)
    })
}

fn run_archive_rebuild_plan(inputs: ArchiveRebuildInputs) -> Result<Vec<String>> {
    const WORKER: &str = "archive rebuild plan";
    preflight_archive_rebuild_plan_volumes(&inputs)?;
    let volume = detect_volume_id(&inputs.records_path)
        .ok()
        .or_else(|| parent_volume(&inputs.records_path));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_archive_rebuild_plan_access_checked(&inputs, || cancellation.check())?;
        cancellation.check()?;
        Ok(plan_archive_rebuilds(&inputs).as_tsv_lines())
    })
}

fn run_archive_migration<T>(
    archive: PathBuf,
    backup_dir: PathBuf,
    worker: &'static str,
    migrate: impl FnOnce(PathBuf, PathBuf, &Cancellation) -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    preflight_archive_migration_volumes(&archive, &backup_dir, worker)?;
    let volume = detect_volume_id(&archive)
        .ok()
        .or_else(|| parent_volume(&archive));
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access =
            retain_archive_migration_access_checked(&archive, &backup_dir, worker, || {
                cancellation.check()
            })?;
        cancellation.check()?;
        migrate(archive, backup_dir, &cancellation)
    })
}

fn run_columns_rebuild_plan(records: PathBuf, columns: PathBuf) -> Result<String> {
    const WORKER: &str = "columns rebuild plan";
    preflight_columns_rebuild_plan_volumes(&records, &columns)?;
    let volume = detect_volume_id(&records)
        .ok()
        .or_else(|| parent_volume(&records));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_columns_rebuild_plan_access_checked(&records, &columns, || {
            cancellation.check()
        })?;
        cancellation.check()?;
        Ok(plan_columns_archive_rebuild(records, columns).as_tsv())
    })
}

fn run_columns_rebuild(
    records: PathBuf,
    columns: PathBuf,
    backup_dir: PathBuf,
) -> Result<ColumnsArchiveRebuild> {
    const WORKER: &str = "columns rebuild";
    preflight_columns_rebuild_volumes(&records, &columns, &backup_dir)?;
    let volume = detect_volume_id(&records)
        .ok()
        .or_else(|| parent_volume(&records));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access =
            retain_columns_rebuild_access_checked(&records, &columns, &backup_dir, || {
                cancellation.check()
            })?;
        cancellation.check()?;
        rebuild_columns_archive(records, columns, backup_dir)
    })
}

fn run_derived_sidecar_rebuild_plan(
    records: PathBuf,
    kind: SidecarKind,
    sidecar: PathBuf,
) -> Result<String> {
    const WORKER: &str = "derived sidecar rebuild plan";
    preflight_derived_sidecar_rebuild_plan_volumes(&records, &sidecar)?;
    let volume = detect_volume_id(&records)
        .ok()
        .or_else(|| parent_volume(&records));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access =
            retain_derived_sidecar_rebuild_plan_access_checked(&records, &sidecar, || {
                cancellation.check()
            })?;
        cancellation.check()?;
        Ok(plan_derived_sidecar_rebuild(records, kind, sidecar).as_tsv())
    })
}

fn run_sidecar_recovery_plan(
    records: PathBuf,
    sidecars: SidecarPaths,
) -> Result<SidecarRecoveryPlan> {
    const WORKER: &str = "sidecar repair plan";
    preflight_sidecar_recovery_plan_volumes(&records, &sidecars)?;
    let volume = detect_volume_id(&records)
        .ok()
        .or_else(|| parent_volume(&records));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_sidecar_recovery_plan_access_checked(&records, &sidecars, || {
            cancellation.check()
        })?;
        cancellation.check()?;
        plan_sidecar_recovery_checked(&records, &sidecars, || cancellation.check())
    })
}

fn retain_record_sidecar_build_access_checked(
    records: &Path,
    output: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let output_probe = write_probe_path(output)?.to_path_buf();
    check_control()?;
    let records_worker = format!("{worker} records");
    let output_worker = format!("{worker} output");
    Ok(vec![
        preflight_access_scope_checked(
            records,
            AccessIntent::Read,
            &records_worker,
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &output_probe,
            AccessIntent::Write,
            &output_worker,
            &mut check_control,
        )?,
    ])
}

fn preflight_record_sidecar_build_volumes(
    records: &Path,
    output: &Path,
    worker: &str,
) -> Result<()> {
    preflight_volume_access_scope(records, AccessIntent::Read, &format!("{worker} records"))?;
    preflight_volume_access_scope(
        write_probe_path(output)?,
        AccessIntent::Write,
        &format!("{worker} output"),
    )
}

fn build_record_sidecar<T>(
    records: PathBuf,
    output: PathBuf,
    worker: &'static str,
    build: impl FnOnce(PathBuf, Vec<FileRecord>, &Cancellation) -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    preflight_record_sidecar_build_volumes(&records, &output, worker)?;
    let volume = detect_volume_id(&records)
        .ok()
        .or_else(|| parent_volume(&records));
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access =
            retain_record_sidecar_build_access_checked(&records, &output, worker, || {
                cancellation.check()
            })?;
        cancellation.check()?;
        let archive = MmapRecordArchive::open_checked(records, || cancellation.check())?;
        cancellation.check()?;
        let records = archive.records_checked(|| cancellation.check())?;
        cancellation.check()?;
        build(output, records, &cancellation)
    })
}

fn retain_archive_migration_access_checked(
    archive: &Path,
    backup_dir: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let archive_write_probe = write_probe_path(archive)?.to_path_buf();
    check_control()?;
    let backup_write_probe = write_probe_path(backup_dir)?.to_path_buf();
    check_control()?;
    let archive_worker = format!("{worker} archive");
    let backup_worker = format!("{worker} backup");
    Ok(vec![
        preflight_access_scope_checked(
            archive,
            AccessIntent::Read,
            &archive_worker,
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &archive_write_probe,
            AccessIntent::Write,
            &archive_worker,
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &backup_write_probe,
            AccessIntent::Write,
            &backup_worker,
            &mut check_control,
        )?,
    ])
}

fn preflight_archive_migration_volumes(
    archive: &Path,
    backup_dir: &Path,
    worker: &str,
) -> Result<()> {
    preflight_volume_access_scope(archive, AccessIntent::Read, &format!("{worker} archive"))?;
    preflight_volume_access_scope(
        write_probe_path(archive)?,
        AccessIntent::Write,
        &format!("{worker} archive"),
    )?;
    preflight_volume_access_scope(
        write_probe_path(backup_dir)?,
        AccessIntent::Write,
        &format!("{worker} backup"),
    )
}

fn retain_columns_rebuild_plan_access_checked(
    records: &Path,
    columns: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let column_probe = archive_probe_path(columns).to_path_buf();
    check_control()?;
    Ok(vec![
        preflight_access_scope_checked(
            records,
            AccessIntent::Read,
            "columns rebuild plan records",
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &column_probe,
            AccessIntent::Read,
            "columns rebuild plan columns",
            &mut check_control,
        )?,
    ])
}

fn preflight_columns_rebuild_plan_volumes(records: &Path, columns: &Path) -> Result<()> {
    preflight_volume_access_scope(records, AccessIntent::Read, "columns rebuild plan records")?;
    preflight_volume_access_scope(
        archive_probe_path(columns),
        AccessIntent::Read,
        "columns rebuild plan columns",
    )
}

fn retain_columns_rebuild_access_checked(
    records: &Path,
    columns: &Path,
    backup_dir: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let columns_read_probe = archive_probe_path(columns).to_path_buf();
    check_control()?;
    let columns_write_probe = write_probe_path(columns)?.to_path_buf();
    check_control()?;
    let backup_write_probe = write_probe_path(backup_dir)?.to_path_buf();
    check_control()?;
    Ok(vec![
        preflight_access_scope_checked(
            records,
            AccessIntent::Read,
            "columns rebuild records",
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &columns_read_probe,
            AccessIntent::Read,
            "columns rebuild columns",
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &columns_write_probe,
            AccessIntent::Write,
            "columns rebuild output",
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &backup_write_probe,
            AccessIntent::Write,
            "columns rebuild backup",
            &mut check_control,
        )?,
    ])
}

fn preflight_columns_rebuild_volumes(
    records: &Path,
    columns: &Path,
    backup_dir: &Path,
) -> Result<()> {
    preflight_volume_access_scope(records, AccessIntent::Read, "columns rebuild records")?;
    preflight_volume_access_scope(
        archive_probe_path(columns),
        AccessIntent::Read,
        "columns rebuild columns",
    )?;
    preflight_volume_access_scope(
        write_probe_path(columns)?,
        AccessIntent::Write,
        "columns rebuild output",
    )?;
    preflight_volume_access_scope(
        write_probe_path(backup_dir)?,
        AccessIntent::Write,
        "columns rebuild backup",
    )
}

fn retain_derived_sidecar_rebuild_plan_access_checked(
    records: &Path,
    sidecar: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let sidecar_probe = archive_probe_path(sidecar).to_path_buf();
    check_control()?;
    Ok(vec![
        preflight_access_scope_checked(
            records,
            AccessIntent::Read,
            "derived sidecar rebuild plan records",
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &sidecar_probe,
            AccessIntent::Read,
            "derived sidecar rebuild plan sidecar",
            &mut check_control,
        )?,
    ])
}

fn preflight_derived_sidecar_rebuild_plan_volumes(records: &Path, sidecar: &Path) -> Result<()> {
    preflight_volume_access_scope(
        records,
        AccessIntent::Read,
        "derived sidecar rebuild plan records",
    )?;
    preflight_volume_access_scope(
        archive_probe_path(sidecar),
        AccessIntent::Read,
        "derived sidecar rebuild plan sidecar",
    )
}

fn retain_archive_rebuild_plan_access_checked(
    inputs: &ArchiveRebuildInputs,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::new();
    for (path, worker) in archive_rebuild_plan_read_paths(inputs) {
        check_control()?;
        let probe = path.to_path_buf();
        check_control()?;
        guards.push(preflight_access_scope_checked(
            &probe,
            AccessIntent::Read,
            worker,
            &mut check_control,
        )?);
    }
    check_control()?;
    Ok(guards)
}

fn preflight_archive_rebuild_plan_volumes(inputs: &ArchiveRebuildInputs) -> Result<()> {
    for (path, worker) in archive_rebuild_plan_read_paths(inputs) {
        preflight_volume_access_scope(path, AccessIntent::Read, worker)?;
    }
    Ok(())
}

fn archive_rebuild_plan_read_paths(inputs: &ArchiveRebuildInputs) -> Vec<(&Path, &'static str)> {
    let mut paths = vec![
        (
            archive_probe_path(&inputs.records_path),
            "archive rebuild plan records",
        ),
        (
            archive_probe_path(&inputs.columns_path),
            "archive rebuild plan columns",
        ),
        (
            archive_probe_path(&inputs.metadata_path),
            "archive rebuild plan metadata",
        ),
        (
            archive_probe_path(&inputs.prefixes_path),
            "archive rebuild plan prefixes",
        ),
        (
            archive_probe_path(&inputs.substrings_path),
            "archive rebuild plan substrings",
        ),
        (
            archive_probe_path(&inputs.fuzzy_path),
            "archive rebuild plan fuzzy",
        ),
        (
            archive_probe_path(&inputs.dictionary_path),
            "archive rebuild plan dictionary",
        ),
        (
            archive_probe_path(&inputs.content_path),
            "archive rebuild plan content",
        ),
        (
            archive_probe_path(&inputs.manifest_path),
            "archive rebuild plan manifest",
        ),
    ];
    paths.extend(inputs.discovered_content_archives.iter().map(|archive| {
        (
            archive_probe_path(&archive.path),
            "archive rebuild plan discovered content",
        )
    }));
    paths
}

fn retain_derived_sidecar_rebuild_access_checked(
    records: &Path,
    sidecar: &Path,
    backup_dir: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let sidecar_write_probe = write_probe_path(sidecar)?.to_path_buf();
    check_control()?;
    let backup_write_probe = write_probe_path(backup_dir)?.to_path_buf();
    check_control()?;
    Ok(vec![
        preflight_access_scope_checked(
            records,
            AccessIntent::Read,
            "derived sidecar rebuild records",
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &sidecar_write_probe,
            AccessIntent::Write,
            "derived sidecar rebuild output",
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &backup_write_probe,
            AccessIntent::Write,
            "derived sidecar rebuild backup",
            &mut check_control,
        )?,
    ])
}

fn preflight_derived_sidecar_rebuild_volumes(
    records: &Path,
    sidecar: &Path,
    backup_dir: &Path,
) -> Result<()> {
    preflight_volume_access_scope(
        records,
        AccessIntent::Read,
        "derived sidecar rebuild records",
    )?;
    preflight_volume_access_scope(
        write_probe_path(sidecar)?,
        AccessIntent::Write,
        "derived sidecar rebuild output",
    )?;
    preflight_volume_access_scope(
        write_probe_path(backup_dir)?,
        AccessIntent::Write,
        "derived sidecar rebuild backup",
    )
}

fn retain_sidecar_recovery_plan_access_checked(
    records: &Path,
    sidecars: &SidecarPaths,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let mut guards = vec![preflight_access_scope_checked(
        records,
        AccessIntent::Read,
        "sidecar repair plan records",
        &mut check_control,
    )?];
    for path in sidecar_paths(sidecars) {
        check_control()?;
        guards.push(preflight_access_scope_checked(
            archive_probe_path(path),
            AccessIntent::Read,
            "sidecar repair plan sidecar",
            &mut check_control,
        )?);
    }
    check_control()?;
    Ok(guards)
}

fn preflight_sidecar_recovery_plan_volumes(records: &Path, sidecars: &SidecarPaths) -> Result<()> {
    preflight_volume_access_scope(records, AccessIntent::Read, "sidecar repair plan records")?;
    for path in sidecar_paths(sidecars) {
        preflight_volume_access_scope(
            archive_probe_path(path),
            AccessIntent::Read,
            "sidecar repair plan sidecar",
        )?;
    }
    Ok(())
}

fn retain_sidecar_recovery_access_checked(
    records: &Path,
    sidecars: &SidecarPaths,
    quarantine: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let quarantine_probe = write_probe_path(quarantine)?.to_path_buf();
    check_control()?;
    let mut guards = vec![
        preflight_access_scope_checked(
            records,
            AccessIntent::Read,
            "sidecar repair records",
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            &quarantine_probe,
            AccessIntent::Write,
            "sidecar repair quarantine",
            &mut check_control,
        )?,
    ];
    for path in sidecar_paths(sidecars) {
        check_control()?;
        let output_probe = write_probe_path(path)?.to_path_buf();
        check_control()?;
        guards.push(preflight_access_scope_checked(
            &output_probe,
            AccessIntent::Write,
            "sidecar repair output",
            &mut check_control,
        )?);
    }
    check_control()?;
    Ok(guards)
}

fn preflight_sidecar_recovery_volumes(
    records: &Path,
    sidecars: &SidecarPaths,
    quarantine: &Path,
) -> Result<()> {
    preflight_volume_access_scope(records, AccessIntent::Read, "sidecar repair records")?;
    preflight_volume_access_scope(
        write_probe_path(quarantine)?,
        AccessIntent::Write,
        "sidecar repair quarantine",
    )?;
    for path in sidecar_paths(sidecars) {
        preflight_volume_access_scope(
            write_probe_path(path)?,
            AccessIntent::Write,
            "sidecar repair output",
        )?;
    }
    Ok(())
}

fn sidecar_paths(sidecars: &SidecarPaths) -> impl Iterator<Item = &Path> {
    [
        sidecars.columns.as_deref(),
        sidecars.metadata.as_deref(),
        sidecars.prefixes.as_deref(),
        sidecars.substrings.as_deref(),
        sidecars.fuzzy.as_deref(),
        sidecars.dictionary.as_deref(),
    ]
    .into_iter()
    .flatten()
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Ok(crate::parent_or_cwd(path)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("archive write path metadata unavailable: {err}"),
        )),
    }
}

fn fail_first_archive_retry_probe_attempt(
    attempt_state: &Path,
    worker: &str,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.check()?;
    let probe = write_probe_path(attempt_state)?.to_path_buf();
    let _access = preflight_access_scope_checked(&probe, AccessIntent::Write, worker, || {
        cancellation.check()
    })?;
    cancellation.check()?;
    let attempts =
        read_archive_retry_probe_attempt_checked(attempt_state, || cancellation.check())?;
    cancellation.check()?;
    write_archive_retry_probe_attempt_checked(attempt_state, attempts + 1, || {
        cancellation.check()
    })?;
    cancellation.check()?;
    if attempts == 0 {
        return Err(GfmError::Format(format!(
            "temporary {worker} retry probe busy"
        )));
    }
    Ok(())
}

fn read_archive_retry_probe_attempt_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<usize> {
    check_control()?;
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(GfmError::io(path, err)),
    };
    check_control()?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        check_control()?;
        let read = file
            .read(&mut buffer)
            .map_err(|err| GfmError::io(path, err))?;
        check_control()?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > 4096 {
            return Ok(0);
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let Ok(value) = std::str::from_utf8(&bytes) else {
        return Ok(0);
    };
    Ok(value.trim().parse::<usize>().unwrap_or(0))
}

fn write_archive_retry_probe_attempt_checked(
    path: &Path,
    attempt: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let encoded = attempt.to_string();
    atomic_write_checked(path, &mut check_control, |writer, check_control| {
        for chunk in encoded.as_bytes().chunks(4096) {
            check_control()?;
            writer
                .write_all(chunk)
                .map_err(|err| GfmError::io(path, err))?;
            check_control()?;
        }
        Ok(())
    })?;
    check_control()?;
    Ok(())
}

fn archive_probe_path(path: &Path) -> &Path {
    crate::parent_or_cwd(path)
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

fn parse_sidecar_paths(
    args: &mut impl Iterator<Item = String>,
    command: &str,
) -> Result<SidecarPaths> {
    Ok(SidecarPaths {
        columns: optional_path_arg(
            args.next(),
            &format!("{command} requires a columns path or -"),
        )?,
        metadata: optional_path_arg(
            args.next(),
            &format!("{command} requires a metadata path or -"),
        )?,
        prefixes: optional_path_arg(
            args.next(),
            &format!("{command} requires a prefixes path or -"),
        )?,
        substrings: optional_path_arg(
            args.next(),
            &format!("{command} requires a substrings path or -"),
        )?,
        fuzzy: optional_path_arg(
            args.next(),
            &format!("{command} requires a fuzzy path or -"),
        )?,
        dictionary: optional_path_arg(
            args.next(),
            &format!("{command} requires a dictionary path or -"),
        )?,
    })
}

fn parse_sidecar_kind(value: Option<String>, command: &str) -> Result<SidecarKind> {
    let value = value.ok_or_else(|| {
        GfmError::Format(format!(
            "{command} requires columns, metadata, prefixes, substrings, fuzzy, or dictionary"
        ))
    })?;
    match value.as_str() {
        "columns" => Ok(SidecarKind::Columns),
        "metadata" => Ok(SidecarKind::Metadata),
        "prefixes" | "prefix" => Ok(SidecarKind::Prefixes),
        "substrings" | "substring" => Ok(SidecarKind::Substrings),
        "fuzzy" => Ok(SidecarKind::Fuzzy),
        "dictionary" => Ok(SidecarKind::Dictionary),
        _ => Err(GfmError::Format(format!(
            "{command} requires columns, metadata, prefixes, substrings, fuzzy, or dictionary"
        ))),
    }
}

fn print_sidecar_recovery_report(report: SidecarRecovery) {
    println!("{}", report.before.as_tsv());
    println!(
        "sidecar-recovery\trebuilt={}\tquarantined={}",
        report.rebuilt_sidecars.len(),
        report.quarantined_sidecars.len()
    );
    println!("{}", report.after.as_tsv());
    print_sidecar_health("invalid-before", &report.before.invalid_sidecars);
    for path in report.quarantined_sidecars {
        println!("quarantined\t{}", path.display());
    }
}

fn print_sidecar_health(label: &str, sidecars: &[SidecarHealth]) {
    for sidecar in sidecars {
        println!(
            "{}\t{}\t{}\t{}",
            label,
            sidecar_kind_name(sidecar.kind),
            sidecar.path.display(),
            sidecar.detail.as_deref().unwrap_or("-")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_read_cancellable_passes_runtime_token_to_reader() {
        let path = std::env::temp_dir().join(format!(
            "gfm-archive-cancellation-token-{}.gfmidx",
            std::process::id()
        ));
        fs::write(&path, b"token-probe").unwrap();

        let result = run_archive_read_cancellable(
            path.clone(),
            "archive cancellation token",
            |_path, cancellation| {
                cancellation.cancel();
                cancellation.check()
            },
        );

        assert_eq!(result, Err(GfmError::Cancelled));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn archive_read_access_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-archive-read-access-cancel");
        let archive = root.join("records.gfmidx");
        fs::write(&archive, b"records").unwrap();

        let result = retain_archive_read_access_checked(&archive, "archive read", || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn record_sidecar_build_access_checked_honors_cancel_before_output_probe() {
        let root = unique_temp_dir("gfm-record-sidecar-build-access-cancel");
        let records = root.join("records.gfmidx");
        let output = root.join("missing").join("records.gfmcols");
        fs::write(&records, b"records").unwrap();

        let result =
            retain_record_sidecar_build_access_checked(&records, &output, "index columns", || {
                Err(GfmError::Cancelled)
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!output.parent().unwrap().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archive_rebuild_plan_access_checked_rechecks_control_across_inputs() {
        let root = unique_temp_dir("gfm-archive-rebuild-plan-access-cancel");
        let inputs = ArchiveRebuildInputs {
            records_path: root.join("records.gfmidx"),
            columns_path: root.join("columns.gfmcols"),
            metadata_path: root.join("metadata.gfmmeta"),
            prefixes_path: root.join("prefixes.gfmprefix"),
            substrings_path: root.join("substrings.gfmsubstr"),
            fuzzy_path: root.join("fuzzy.gfmfuzzy"),
            dictionary_path: root.join("dictionary.gfmdict"),
            content_path: root.join("content.gfmcontent"),
            manifest_path: root.join("content-manifest.tsv"),
            discovered_content_archives: vec![ContentArchiveManifestEntry {
                tier: ContentMergeTier::Warm,
                path: root.join("warm.gfmcontent"),
            }],
        };
        let mut checks = 0usize;

        let result = retain_archive_rebuild_plan_access_checked(&inputs, || {
            checks += 1;
            if checks >= 4 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sidecar_recovery_plan_access_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-sidecar-recovery-plan-access-cancel");
        let records = root.join("records.gfmidx");
        let sidecars = SidecarPaths {
            columns: Some(root.join("columns.gfmcols")),
            metadata: None,
            prefixes: None,
            substrings: None,
            fuzzy: None,
            dictionary: None,
        };
        fs::write(&records, b"records").unwrap();

        let result = retain_sidecar_recovery_plan_access_checked(&records, &sidecars, || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!sidecars.columns.as_ref().unwrap().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sidecar_recovery_access_checked_honors_cancel_before_quarantine_probe() {
        let root = unique_temp_dir("gfm-sidecar-recovery-access-cancel");
        let records = root.join("records.gfmidx");
        let quarantine = root.join("missing").join("quarantine");
        let sidecars = SidecarPaths {
            columns: Some(root.join("columns.gfmcols")),
            metadata: None,
            prefixes: None,
            substrings: None,
            fuzzy: None,
            dictionary: None,
        };
        fs::write(&records, b"records").unwrap();

        let result =
            retain_sidecar_recovery_access_checked(&records, &sidecars, &quarantine, || {
                Err(GfmError::Cancelled)
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!quarantine.exists());
        assert!(!quarantine.parent().unwrap().exists());
        assert!(!sidecars.columns.as_ref().unwrap().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sidecar_recovery_access_checked_can_cancel_between_output_probes() {
        let root = unique_temp_dir("gfm-sidecar-recovery-output-access-cancel");
        let records = root.join("records.gfmidx");
        let quarantine = root.join("quarantine");
        let sidecars = SidecarPaths {
            columns: Some(root.join("columns.gfmcols")),
            metadata: Some(root.join("metadata.gfmmeta")),
            prefixes: None,
            substrings: None,
            fuzzy: None,
            dictionary: None,
        };
        fs::write(&records, b"records").unwrap();
        fs::create_dir(&quarantine).unwrap();
        let mut checks = 0usize;

        let result =
            retain_sidecar_recovery_access_checked(&records, &sidecars, &quarantine, || {
                checks += 1;
                if checks >= 4 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 4);
        assert!(!sidecars.columns.as_ref().unwrap().exists());
        assert!(!sidecars.metadata.as_ref().unwrap().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn record_sidecar_build_cancellable_passes_runtime_token_to_writer() {
        let root = std::env::temp_dir().join(format!(
            "gfm-record-sidecar-cancellation-token-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let records = root.join("records.gfmidx");
        let output = root.join("records.gfmprefix");
        gfm_store::write_records(
            &records,
            &[FileRecord {
                id: FileId::new(VolumeId(1), 1),
                parent: None,
                path: PathBuf::from("/tmp/project.md"),
                name: "project.md".to_string(),
                kind: gfm_types::FileKind::File,
                len: 7,
                mode: 0o100644,
                owner: 501,
                group: 20,
                xattrs_digest: 0,
                created: None,
                modified: None,
                changed: None,
                hidden: false,
                tags: Vec::new(),
                finder_comment: None,
            }],
        )
        .unwrap();

        let result = build_record_sidecar(
            records,
            output.clone(),
            "archive cancellation token",
            |output, records, cancellation| {
                let postings = prefix_postings_from_records(&records);
                cancellation.cancel();
                write_prefix_postings_checked(output, &postings, || cancellation.check())
            },
        );

        assert_eq!(result, Err(GfmError::Cancelled));
        assert!(!output.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
