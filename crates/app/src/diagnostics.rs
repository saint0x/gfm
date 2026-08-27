use crate::access::{preflight_access_scope, ScopedAccessGuard};
use crate::runtime::{run_scheduled_volume_task_cancellable, run_volume_task_cancellable};
use crate::{
    config_store, detect_volume_id, existing_read_probe_path, parent_volume,
    parse_required_scheduling_pressure, preflight_config_write, required_path,
};
use gfm_diagnostics::{
    export_operator_trace, inspect_storage, plan_index_recovery, rebuild_index_cancellable,
    recover_index_cancellable, select_parity_baseline, PersistentIndexRecoverySpec, RebuildSpec,
    StorageInspection,
};
use gfm_index::PersistentIndexRecovery;
use gfm_jobs::Priority;
use gfm_mac::AccessIntent;
use gfm_types::{GfmError, Result};
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "diagnostics-index-rebuild" => {
            let root = required_path(
                args.next(),
                "diagnostics-index-rebuild requires a root path",
            )?;
            let records = required_path(
                args.next(),
                "diagnostics-index-rebuild requires a records path",
            )?;
            let spec = match args.next() {
                Some(content) => RebuildSpec::with_content(root, records, PathBuf::from(content)),
                None => RebuildSpec::records(root, records),
            };
            let volume = detect_volume_id(&spec.root)
                .ok()
                .or_else(|| parent_volume(&spec.records_path));
            let _access = retain_rebuild_access(&spec)?;
            let report = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "index rebuild",
                move |cancellation| rebuild_index_cancellable(&spec, &cancellation),
            )?;
            print_index_rebuild_report(report);
        }
        "diagnostics-index-rebuild-adaptive" => {
            let root = required_path(
                args.next(),
                "diagnostics-index-rebuild-adaptive requires a root path",
            )?;
            let records = required_path(
                args.next(),
                "diagnostics-index-rebuild-adaptive requires a records path",
            )?;
            let pressure = parse_required_scheduling_pressure(args, "index rebuild")?;
            let spec = match args.next() {
                Some(content) => RebuildSpec::with_content(root, records, PathBuf::from(content)),
                None => RebuildSpec::records(root, records),
            };
            let volume = detect_volume_id(&spec.root)
                .ok()
                .or_else(|| parent_volume(&spec.records_path));
            let _access = retain_rebuild_access(&spec)?;
            let outcome = run_scheduled_volume_task_cancellable(
                volume,
                Priority::Background,
                "index rebuild",
                pressure,
                move |cancellation| rebuild_index_cancellable(&spec, &cancellation),
            )?;
            if outcome.deferred {
                eprintln!(
                    "index-rebuild-deferred\taction={:?}",
                    outcome.scheduling_action
                );
            } else {
                let report = outcome.result.ok_or_else(|| {
                    GfmError::Format("index rebuild ran without a report".to_string())
                })?;
                eprintln!("index-rebuild-action\t{:?}", outcome.scheduling_action);
                print_index_rebuild_report(report);
            }
        }
        "diagnostics-index-recovery-plan" => {
            let root = required_path(
                args.next(),
                "diagnostics-index-recovery-plan requires a root path",
            )?;
            let records = required_path(
                args.next(),
                "diagnostics-index-recovery-plan requires a records path",
            )?;
            let state = required_path(
                args.next(),
                "diagnostics-index-recovery-plan requires a state path",
            )?;
            let quarantine = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| records.with_extension("quarantine"));
            let spec = PersistentIndexRecoverySpec::new(root, records, state, quarantine);
            let _access = retain_recovery_plan_access(&spec)?;
            println!("{}", plan_index_recovery(&spec).as_tsv());
        }
        "diagnostics-index-recover" => {
            let root = required_path(
                args.next(),
                "diagnostics-index-recover requires a root path",
            )?;
            let records = required_path(
                args.next(),
                "diagnostics-index-recover requires a records path",
            )?;
            let state = required_path(
                args.next(),
                "diagnostics-index-recover requires a state path",
            )?;
            let quarantine = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| records.with_extension("quarantine"));
            let spec = PersistentIndexRecoverySpec::new(root, records, state, quarantine);
            let volume = detect_volume_id(&spec.root)
                .ok()
                .or_else(|| parent_volume(&spec.records_path));
            let _access = retain_recovery_access(&spec)?;
            let report = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "persistent index repair",
                move |cancellation| recover_index_cancellable(&spec, &cancellation),
            )?;
            print_persistent_index_recovery_report(report);
        }
        "diagnostics-index-recover-adaptive" => {
            let root = required_path(
                args.next(),
                "diagnostics-index-recover-adaptive requires a root path",
            )?;
            let records = required_path(
                args.next(),
                "diagnostics-index-recover-adaptive requires a records path",
            )?;
            let state = required_path(
                args.next(),
                "diagnostics-index-recover-adaptive requires a state path",
            )?;
            let pressure = parse_required_scheduling_pressure(args, "persistent index repair")?;
            let quarantine = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| records.with_extension("quarantine"));
            let spec = PersistentIndexRecoverySpec::new(root, records, state, quarantine);
            let volume = detect_volume_id(&spec.root)
                .ok()
                .or_else(|| parent_volume(&spec.records_path));
            let _access = retain_recovery_access(&spec)?;
            let outcome = run_scheduled_volume_task_cancellable(
                volume,
                Priority::Background,
                "persistent index repair",
                pressure,
                move |cancellation| recover_index_cancellable(&spec, &cancellation),
            )?;
            if outcome.deferred {
                eprintln!(
                    "persistent-index-recovery-deferred\taction={:?}",
                    outcome.scheduling_action
                );
            } else {
                let report = outcome.result.ok_or_else(|| {
                    GfmError::Format("persistent index repair ran without a report".to_string())
                })?;
                eprintln!(
                    "persistent-index-recovery-action\t{:?}",
                    outcome.scheduling_action
                );
                print_persistent_index_recovery_report(report);
            }
        }
        "diagnostics-trace-export" => {
            let output = required_path(
                args.next(),
                "diagnostics-trace-export requires an output path",
            )?;
            let report = export_operator_trace(output)?;
            println!("{}\t{}", report.path.display(), report.bytes_written);
        }
        "diagnostics-parity-baseline" => {
            let store = config_store(args.next())?;
            let baseline = required_path(
                args.next(),
                "diagnostics-parity-baseline requires a baseline root",
            )?;
            let macos_build = args.next().ok_or_else(|| {
                GfmError::Format("diagnostics-parity-baseline requires a macOS build".to_string())
            })?;
            let _config_access = preflight_config_write(&store, "diagnostics parity config")?;
            let _baseline_access = preflight_access_scope(
                existing_read_probe_path(&baseline),
                AccessIntent::Read,
                "diagnostics parity baseline",
            )?;
            let report = select_parity_baseline(&store, baseline, macos_build)?;
            println!(
                "{}\t{}\t{}",
                report.config_path.display(),
                report.baseline_root.display(),
                report.macos_build
            );
        }
        "diagnostics-storage-inspect" => {
            let storage = required_path(
                args.next(),
                "diagnostics-storage-inspect requires a storage path",
            )?;
            match inspect_storage(storage)? {
                StorageInspection::Records(report) => println!(
                    "records\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    report.path.display(),
                    report.bytes,
                    report.records,
                    report.files,
                    report.directories,
                    report.symlinks,
                    report.hidden,
                    report.tagged
                ),
                StorageInspection::Content(report) => println!(
                    "content\t{}\t{}\t{}",
                    report.path.display(),
                    report.bytes,
                    report.terms
                ),
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn retain_rebuild_access(spec: &RebuildSpec) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::new();
    guards.push(preflight_access_scope(
        &spec.root,
        AccessIntent::Index,
        "index rebuild root",
    )?);
    guards.push(preflight_access_scope(
        write_probe_path(&spec.records_path),
        AccessIntent::Write,
        "index rebuild records",
    )?);
    if let Some(content_path) = &spec.content_path {
        guards.push(preflight_access_scope(
            write_probe_path(content_path),
            AccessIntent::Write,
            "index rebuild content",
        )?);
    }
    Ok(guards)
}

fn retain_recovery_plan_access(
    spec: &PersistentIndexRecoverySpec,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![preflight_access_scope(
        &spec.root,
        AccessIntent::Index,
        "persistent index repair root",
    )?])
}

fn retain_recovery_access(spec: &PersistentIndexRecoverySpec) -> Result<Vec<ScopedAccessGuard>> {
    let guards = vec![
        preflight_access_scope(
            &spec.root,
            AccessIntent::Index,
            "persistent index repair root",
        )?,
        preflight_access_scope(
            write_probe_path(&spec.records_path),
            AccessIntent::Write,
            "persistent index repair records",
        )?,
        preflight_access_scope(
            write_probe_path(&spec.state_path),
            AccessIntent::Write,
            "persistent index repair state",
        )?,
        preflight_access_scope(
            write_probe_path(&spec.quarantine_dir),
            AccessIntent::Write,
            "persistent index repair quarantine",
        )?,
    ];
    Ok(guards)
}

fn write_probe_path(path: &Path) -> &Path {
    if path.is_dir() {
        return path;
    }
    path.parent().unwrap_or(path)
}

fn print_index_rebuild_report(report: gfm_diagnostics::RebuildReport) {
    println!(
        "{}\t{}\t{}\t{}\t{}",
        report.root.display(),
        report.records_path.display(),
        report
            .content_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string()),
        report.records,
        report.content_indexed
    );
    if report.inaccessible != 0 {
        eprintln!("inaccessible\t{}", report.inaccessible);
    }
}

fn print_persistent_index_recovery_report(report: PersistentIndexRecovery) {
    println!("{}", report.before.as_tsv());
    println!(
        "persistent-index-recovery\trebuilt-records={}\trebuilt-state={}\tquarantined-records={}",
        report.rebuilt_records,
        report.rebuilt_state,
        report
            .quarantined_records_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!("{}", report.after.as_tsv());
}
