use crate::access::{preflight_access_scope, preflight_volume_access_scope, ScopedAccessGuard};
use crate::runtime::{
    run_scheduled_volume_task_cancellable_with_volume_and_payload_path,
    run_volume_task_cancellable, run_volume_task_cancellable_with_payload_path,
};
use crate::{
    config_store, config_write_probe_path, detect_volume_id, existing_read_probe_path,
    parent_volume, parse_required_scheduling_pressure, preflight_config_write, required_path,
};
use gfm_config::ConfigStore;
use gfm_diagnostics::{
    export_operator_trace, inspect_storage, plan_index_recovery_cancellable,
    rebuild_index_cancellable, recover_index_cancellable, select_parity_baseline,
    PersistentIndexRecoverySpec, RebuildSpec, StorageInspection,
};
use gfm_index::{PersistentIndexPlan, PersistentIndexRecovery};
use gfm_jobs::{Priority, SchedulingAction};
use gfm_mac::AccessIntent;
use gfm_types::{GfmError, Result};
use std::fs;
use std::io;
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
            preflight_rebuild_volumes(&spec)?;
            let report = run_volume_task_cancellable_with_payload_path(
                volume,
                Priority::Visible,
                "index rebuild",
                spec.records_path.clone(),
                move |cancellation| {
                    cancellation.check()?;
                    let _access = retain_rebuild_access(&spec)?;
                    cancellation.check()?;
                    rebuild_index_cancellable(&spec, &cancellation)
                },
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
            let outcome =
                if pressure.decide(Priority::Background, 1, 1).action == SchedulingAction::Defer {
                    run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
                        Priority::Background,
                        "index rebuild",
                        pressure,
                        || Ok(None),
                        spec.records_path.clone(),
                        move |cancellation| rebuild_index_cancellable(&spec, &cancellation),
                    )?
                } else {
                    let volume_spec = spec.clone();
                    run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
                        Priority::Background,
                        "index rebuild",
                        pressure,
                        move || {
                            preflight_rebuild_volumes(&volume_spec)?;
                            Ok(detect_volume_id(&volume_spec.root)
                                .ok()
                                .or_else(|| parent_volume(&volume_spec.records_path)))
                        },
                        spec.records_path.clone(),
                        move |cancellation| {
                            let _access = retain_rebuild_access(&spec)?;
                            rebuild_index_cancellable(&spec, &cancellation)
                        },
                    )?
                };
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
            println!("{}", run_recovery_plan(spec)?.as_tsv());
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
            preflight_recovery_volumes(&spec)?;
            let report = run_volume_task_cancellable_with_payload_path(
                volume,
                Priority::Visible,
                "persistent index repair",
                spec.state_path.clone(),
                move |cancellation| {
                    cancellation.check()?;
                    let _access = retain_recovery_access(&spec)?;
                    cancellation.check()?;
                    recover_index_cancellable(&spec, &cancellation)
                },
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
            let outcome =
                if pressure.decide(Priority::Background, 1, 1).action == SchedulingAction::Defer {
                    run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
                        Priority::Background,
                        "persistent index repair",
                        pressure,
                        || Ok(None),
                        spec.state_path.clone(),
                        move |cancellation| recover_index_cancellable(&spec, &cancellation),
                    )?
                } else {
                    let volume_spec = spec.clone();
                    run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
                        Priority::Background,
                        "persistent index repair",
                        pressure,
                        move || {
                            preflight_recovery_volumes(&volume_spec)?;
                            Ok(detect_volume_id(&volume_spec.root)
                                .ok()
                                .or_else(|| parent_volume(&volume_spec.records_path)))
                        },
                        spec.state_path.clone(),
                        move |cancellation| {
                            let _access = retain_recovery_access(&spec)?;
                            recover_index_cancellable(&spec, &cancellation)
                        },
                    )?
                };
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
            println!("{}", run_trace_export(output)?);
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
            println!("{}", run_parity_baseline(store, baseline, macos_build)?);
        }
        "diagnostics-storage-inspect" => {
            let storage = required_path(
                args.next(),
                "diagnostics-storage-inspect requires a storage path",
            )?;
            println!("{}", run_storage_inspect(storage)?);
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
        write_probe_path(&spec.records_path)?,
        AccessIntent::Write,
        "index rebuild records",
    )?);
    if let Some(content_path) = &spec.content_path {
        guards.push(preflight_access_scope(
            write_probe_path(content_path)?,
            AccessIntent::Write,
            "index rebuild content",
        )?);
    }
    Ok(guards)
}

fn preflight_rebuild_volumes(spec: &RebuildSpec) -> Result<()> {
    preflight_volume_access_scope(&spec.root, AccessIntent::Index, "index rebuild root")?;
    preflight_volume_access_scope(
        write_probe_path(&spec.records_path)?,
        AccessIntent::Write,
        "index rebuild records",
    )?;
    if let Some(content_path) = &spec.content_path {
        preflight_volume_access_scope(
            write_probe_path(content_path)?,
            AccessIntent::Write,
            "index rebuild content",
        )?;
    }
    Ok(())
}

fn run_recovery_plan(spec: PersistentIndexRecoverySpec) -> Result<PersistentIndexPlan> {
    const WORKER: &str = "persistent index repair plan";
    preflight_volume_access_scope(&spec.root, AccessIntent::Index, WORKER)?;
    let volume = detect_volume_id(&spec.root)
        .ok()
        .or_else(|| parent_volume(&spec.records_path));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = retain_recovery_plan_access(&spec)?;
        cancellation.check()?;
        plan_index_recovery_cancellable(&spec, &cancellation)
    })
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
            write_probe_path(&spec.records_path)?,
            AccessIntent::Write,
            "persistent index repair records",
        )?,
        preflight_access_scope(
            write_probe_path(&spec.state_path)?,
            AccessIntent::Write,
            "persistent index repair state",
        )?,
        preflight_access_scope(
            write_probe_path(&spec.quarantine_dir)?,
            AccessIntent::Write,
            "persistent index repair quarantine",
        )?,
    ];
    Ok(guards)
}

fn preflight_recovery_volumes(spec: &PersistentIndexRecoverySpec) -> Result<()> {
    preflight_volume_access_scope(
        &spec.root,
        AccessIntent::Index,
        "persistent index repair root",
    )?;
    preflight_volume_access_scope(
        write_probe_path(&spec.records_path)?,
        AccessIntent::Write,
        "persistent index repair records",
    )?;
    preflight_volume_access_scope(
        write_probe_path(&spec.state_path)?,
        AccessIntent::Write,
        "persistent index repair state",
    )?;
    preflight_volume_access_scope(
        write_probe_path(&spec.quarantine_dir)?,
        AccessIntent::Write,
        "persistent index repair quarantine",
    )?;
    Ok(())
}

fn run_trace_export(output: PathBuf) -> Result<String> {
    const WORKER: &str = "diagnostics trace export";
    let output_probe = write_probe_path(&output)?.to_path_buf();
    preflight_volume_access_scope(&output_probe, AccessIntent::Write, WORKER)?;
    let volume = parent_volume(&output_probe);
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access =
            preflight_access_scope(write_probe_path(&output)?, AccessIntent::Write, WORKER)?;
        cancellation.check()?;
        let report = export_operator_trace(output)?;
        Ok(format!(
            "{}\t{}",
            report.path.display(),
            report.bytes_written
        ))
    })
}

fn run_parity_baseline(
    store: ConfigStore,
    baseline: PathBuf,
    macos_build: String,
) -> Result<String> {
    preflight_volume_access_scope(
        config_write_probe_path(store.path())?,
        AccessIntent::Write,
        "diagnostics parity config",
    )?;
    preflight_volume_access_scope(
        existing_read_probe_path(&baseline)?,
        AccessIntent::Read,
        "diagnostics parity baseline",
    )?;
    let baseline_probe = existing_read_probe_path(&baseline)?.to_path_buf();
    let volume = parent_volume(config_write_probe_path(store.path())?)
        .or_else(|| parent_volume(&baseline_probe));
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "diagnostics parity baseline",
        move |cancellation| {
            cancellation.check()?;
            let _config_access = preflight_config_write(&store, "diagnostics parity config")?;
            let _baseline_access = preflight_access_scope(
                existing_read_probe_path(&baseline)?,
                AccessIntent::Read,
                "diagnostics parity baseline",
            )?;
            cancellation.check()?;
            let report = select_parity_baseline(&store, baseline, macos_build)?;
            Ok(format!(
                "{}\t{}\t{}",
                report.config_path.display(),
                report.baseline_root.display(),
                report.macos_build
            ))
        },
    )
}

fn run_storage_inspect(storage: PathBuf) -> Result<String> {
    const WORKER: &str = "diagnostics storage";
    preflight_volume_access_scope(&storage, AccessIntent::Read, WORKER)?;
    let volume = detect_volume_id(&storage)
        .ok()
        .or_else(|| parent_volume(&storage));
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_access_scope(&storage, AccessIntent::Read, WORKER)?;
        cancellation.check()?;
        match inspect_storage(storage)? {
            StorageInspection::Records(report) => Ok(format!(
                "records\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                report.path.display(),
                report.bytes,
                report.records,
                report.files,
                report.directories,
                report.symlinks,
                report.hidden,
                report.tagged
            )),
            StorageInspection::Content(report) => Ok(format!(
                "content\t{}\t{}\t{}",
                report.path.display(),
                report.bytes,
                report.terms
            )),
        }
    })
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Ok(crate::parent_or_cwd(path)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("diagnostics write path metadata unavailable: {err}"),
        )),
    }
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
