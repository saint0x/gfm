use crate::runtime::{run_scheduled_volume_task, run_volume_task};
use crate::{
    config_store, detect_volume_id, parent_volume, parse_required_scheduling_pressure,
    required_path,
};
use gfm_diagnostics::{
    export_operator_trace, inspect_storage, plan_index_recovery, rebuild_index, recover_index,
    select_parity_baseline, PersistentIndexRecoverySpec, RebuildSpec, StorageInspection,
};
use gfm_index::PersistentIndexRecovery;
use gfm_jobs::Priority;
use gfm_types::{GfmError, Result};
use std::path::PathBuf;

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
            let report = run_volume_task(volume, Priority::Visible, "index rebuild", move || {
                rebuild_index(&spec)
            })?;
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
            let outcome = run_scheduled_volume_task(
                volume,
                Priority::Background,
                "index rebuild",
                pressure,
                move || rebuild_index(&spec),
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
            let report = run_volume_task(
                volume,
                Priority::Visible,
                "persistent index repair",
                move || recover_index(&spec),
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
            let outcome = run_scheduled_volume_task(
                volume,
                Priority::Background,
                "persistent index repair",
                pressure,
                move || recover_index(&spec),
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
