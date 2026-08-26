use crate::runtime::{run_scheduled_volume_task_cancellable, run_volume_task_cancellable};
use crate::{
    detect_volume_id, optional_path_arg, parent_volume, parse_required_scheduling_pressure,
    parse_u64_arg, required_path,
};
use gfm_index::{ContentArchiveManifestEntry, ContentMergeTier};
use gfm_jobs::Priority;
use gfm_store::{
    dictionary_term_report_from_records, fuzzy_postings_from_records, inspect_archive_schema,
    metadata_postings_from_records, migrate_content_archive, migrate_metadata_archive,
    migrate_record_archive, plan_archive_rebuilds, plan_columns_archive_rebuild,
    plan_content_archive_migration, plan_derived_sidecar_rebuild, plan_metadata_archive_migration,
    plan_record_archive_migration, plan_sidecar_recovery, prefix_postings_from_records,
    rebuild_columns_archive, rebuild_derived_sidecar_checked, recover_sidecars_checked,
    sidecar_kind_name, substring_postings_from_records, write_dictionary, write_fuzzy_postings,
    write_metadata_postings, write_prefix_postings, write_record_columns, write_substring_postings,
    ArchiveRebuildInputs, ArchiveSchemaKind, MmapRecordArchive, MmapRecordColumns, SidecarHealth,
    SidecarKind, SidecarPaths, SidecarRecovery,
};
use gfm_types::{FileId, GfmError, Result, VolumeId};
use std::path::PathBuf;

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "records-verify" => {
            let records = required_path(args.next(), "records-verify requires a records path")?;
            let archive = MmapRecordArchive::open(records)?;
            println!(
                "records-verify\trecords={}\tbytes={}\tchecksum={}",
                archive.len(),
                archive.mapped_len(),
                if archive.is_checksummed() {
                    "verified"
                } else {
                    "legacy"
                }
            );
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
            println!("{}", inspect_archive_schema(kind, path).as_tsv());
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
            for line in plan_archive_rebuilds(&inputs).as_tsv_lines() {
                println!("{line}");
            }
        }
        "records-migration-plan" => {
            let records = required_path(
                args.next(),
                "records-migration-plan requires a records path",
            )?;
            println!("{}", plan_record_archive_migration(records).as_tsv());
        }
        "records-migrate" => {
            let records = required_path(args.next(), "records-migrate requires a records path")?;
            let backup_dir =
                required_path(args.next(), "records-migrate requires a backup directory")?;
            let migration = migrate_record_archive(records, backup_dir)?;
            println!("{}", migration.as_tsv());
        }
        "content-migration-plan" => {
            let content = required_path(
                args.next(),
                "content-migration-plan requires a content path",
            )?;
            println!("{}", plan_content_archive_migration(content).as_tsv());
        }
        "content-migrate" => {
            let content = required_path(args.next(), "content-migrate requires a content path")?;
            let backup_dir =
                required_path(args.next(), "content-migrate requires a backup directory")?;
            let migration = migrate_content_archive(content, backup_dir)?;
            println!("{}", migration.as_tsv());
        }
        "metadata-migration-plan" => {
            let metadata = required_path(
                args.next(),
                "metadata-migration-plan requires a metadata path",
            )?;
            println!("{}", plan_metadata_archive_migration(metadata).as_tsv());
        }
        "metadata-migrate" => {
            let metadata = required_path(args.next(), "metadata-migrate requires a metadata path")?;
            let backup_dir =
                required_path(args.next(), "metadata-migrate requires a backup directory")?;
            let migration = migrate_metadata_archive(metadata, backup_dir)?;
            println!("{}", migration.as_tsv());
        }
        "columns-rebuild-plan" => {
            let records =
                required_path(args.next(), "columns-rebuild-plan requires a records path")?;
            let columns =
                required_path(args.next(), "columns-rebuild-plan requires a columns path")?;
            println!(
                "{}",
                plan_columns_archive_rebuild(records, columns).as_tsv()
            );
        }
        "columns-rebuild" => {
            let records = required_path(args.next(), "columns-rebuild requires a records path")?;
            let columns = required_path(args.next(), "columns-rebuild requires a columns path")?;
            let backup_dir =
                required_path(args.next(), "columns-rebuild requires a backup directory")?;
            let rebuild = rebuild_columns_archive(records, columns, backup_dir)?;
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
            println!(
                "{}",
                plan_derived_sidecar_rebuild(records, kind, sidecar).as_tsv()
            );
        }
        "derived-sidecar-rebuild" => {
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
            let volume = detect_volume_id(&records)
                .ok()
                .or_else(|| parent_volume(&records));
            let rebuild = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "derived sidecar rebuild",
                move |cancellation| {
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
            let archive = MmapRecordArchive::open(records)?;
            let records = archive.records()?;
            write_record_columns(output, &records)?;
            eprintln!("columns-indexed {} records", records.len());
        }
        "columns-verify" => {
            let columns = required_path(args.next(), "columns-verify requires a columns path")?;
            let archive = MmapRecordColumns::open(columns)?;
            println!(
                "columns-verify\trecords={}\tbytes={}\tchecksum={}",
                archive.len(),
                archive.mapped_len(),
                if archive.is_checksummed() {
                    "verified"
                } else {
                    "legacy"
                }
            );
        }
        "columns-lookup" => {
            let columns = required_path(args.next(), "columns-lookup requires a columns path")?;
            let volume = parse_u64_arg(args.next(), "columns-lookup requires a volume id")?;
            let node = parse_u64_arg(args.next(), "columns-lookup requires a node id")?;
            let archive = MmapRecordColumns::open(columns)?;
            match archive.find(FileId::new(VolumeId(volume), node))? {
                Some(column) => println!(
                    "columns\tfound\tid={}:{}\tname={}\text={}\ttags={}\tcomment={}\tpath={}",
                    column.id.volume.0,
                    column.id.node,
                    column.name,
                    column.extension.as_deref().unwrap_or(""),
                    column.tags.join(","),
                    column.comment.as_deref().unwrap_or(""),
                    column.path
                ),
                None => println!("columns\tmissing\tid={volume}:{node}"),
            }
        }
        "index-metadata" => {
            let records = required_path(args.next(), "index-metadata requires a records path")?;
            let output = required_path(
                args.next(),
                "index-metadata requires an output metadata path",
            )?;
            let archive = MmapRecordArchive::open(records)?;
            let postings = metadata_postings_from_records(&archive.records()?);
            write_metadata_postings(output, &postings)?;
            eprintln!("metadata-indexed {} terms", postings.len());
        }
        "index-dictionary" => {
            let records = required_path(args.next(), "index-dictionary requires a records path")?;
            let output = required_path(
                args.next(),
                "index-dictionary requires an output dictionary path",
            )?;
            let archive = MmapRecordArchive::open(records)?;
            let report = dictionary_term_report_from_records(&archive.records()?);
            write_dictionary(output, &report.terms)?;
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
            let archive = MmapRecordArchive::open(records)?;
            let postings = prefix_postings_from_records(&archive.records()?);
            write_prefix_postings(output, &postings)?;
            eprintln!("prefixes-indexed {} prefixes", postings.len());
        }
        "index-substrings" => {
            let records = required_path(args.next(), "index-substrings requires a records path")?;
            let output = required_path(
                args.next(),
                "index-substrings requires an output substring path",
            )?;
            let archive = MmapRecordArchive::open(records)?;
            let postings = substring_postings_from_records(&archive.records()?);
            write_substring_postings(output, &postings)?;
            eprintln!("substrings-indexed {} grams", postings.len());
        }
        "index-fuzzy" => {
            let records = required_path(args.next(), "index-fuzzy requires a records path")?;
            let output = required_path(args.next(), "index-fuzzy requires an output fuzzy path")?;
            let archive = MmapRecordArchive::open(records)?;
            let postings = fuzzy_postings_from_records(&archive.records()?);
            write_fuzzy_postings(output, &postings)?;
            eprintln!("fuzzy-indexed {} keys", postings.len());
        }
        "sidecar-recovery-plan" => {
            let records =
                required_path(args.next(), "sidecar-recovery-plan requires a records path")?;
            let sidecars = parse_sidecar_paths(args, "sidecar-recovery-plan")?;
            let plan = plan_sidecar_recovery(&records, &sidecars);
            println!("{}", plan.as_tsv());
            print_sidecar_health("invalid", &plan.invalid_sidecars);
        }
        "sidecar-recover" => {
            let records = required_path(args.next(), "sidecar-recover requires a records path")?;
            let quarantine = required_path(
                args.next(),
                "sidecar-recover requires a quarantine directory",
            )?;
            let sidecars = parse_sidecar_paths(args, "sidecar-recover")?;
            let volume = detect_volume_id(&records)
                .ok()
                .or_else(|| parent_volume(&records));
            let report = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "sidecar repair",
                move |cancellation| {
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
            let volume = detect_volume_id(&records)
                .ok()
                .or_else(|| parent_volume(&records));
            let outcome = run_scheduled_volume_task_cancellable(
                volume,
                Priority::Background,
                "sidecar repair",
                pressure,
                move |cancellation| {
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
