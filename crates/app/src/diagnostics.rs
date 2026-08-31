use crate::access::{
    preflight_access_scope_checked_with_volume_report, preflight_volume_access_scope_with_report,
    ScopedAccessGuard,
};
use crate::runtime::{
    run_scheduled_volume_task_cancellable_with_volume_and_payload_path,
    run_volume_task_cancellable, run_volume_task_cancellable_with_payload_path,
};
use crate::{
    config_store, config_write_probe_path, existing_read_probe_path,
    parse_required_scheduling_pressure, required_path,
};
use gfm_config::ConfigStore;
use gfm_diagnostics::{
    export_operator_trace_checked, inspect_storage_checked, plan_index_recovery_cancellable,
    rebuild_index_cancellable, recover_index_cancellable, select_parity_baseline_checked,
    PersistentIndexRecoverySpec, RebuildSpec, StorageInspection,
};
use gfm_index::{PersistentIndexPlan, PersistentIndexRecovery};
use gfm_jobs::{Priority, SchedulingAction};
use gfm_mac::{AccessIntent, VolumeDiscoveryReport};
use gfm_types::{GfmError, Result, VolumeId};
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
            let access_reports = rebuild_access_reports(&spec)?;
            access_reports.preflight_volumes()?;
            let volume = access_reports.first_volume();
            let report = run_volume_task_cancellable_with_payload_path(
                volume,
                Priority::Visible,
                "index rebuild",
                spec.records_path.clone(),
                move |cancellation| {
                    cancellation.check()?;
                    let _access = access_reports.access_checked(|| cancellation.check())?;
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
                    let access_reports = rebuild_access_reports(&spec)?;
                    let volume_access_reports = access_reports.clone();
                    run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
                        Priority::Background,
                        "index rebuild",
                        pressure,
                        move || {
                            volume_access_reports.preflight_volumes()?;
                            Ok(volume_access_reports.first_volume())
                        },
                        spec.records_path.clone(),
                        move |cancellation| {
                            cancellation.check()?;
                            let _access = access_reports.access_checked(|| cancellation.check())?;
                            cancellation.check()?;
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
            let access_reports = recovery_access_reports(&spec)?;
            access_reports.preflight_volumes()?;
            let volume = access_reports.first_volume();
            let report = run_volume_task_cancellable_with_payload_path(
                volume,
                Priority::Visible,
                "persistent index repair",
                spec.state_path.clone(),
                move |cancellation| {
                    cancellation.check()?;
                    let _access = access_reports.access_checked(|| cancellation.check())?;
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
                    let access_reports = recovery_access_reports(&spec)?;
                    let volume_access_reports = access_reports.clone();
                    run_scheduled_volume_task_cancellable_with_volume_and_payload_path(
                        Priority::Background,
                        "persistent index repair",
                        pressure,
                        move || {
                            volume_access_reports.preflight_volumes()?;
                            Ok(volume_access_reports.first_volume())
                        },
                        spec.state_path.clone(),
                        move |cancellation| {
                            cancellation.check()?;
                            let _access = access_reports.access_checked(|| cancellation.check())?;
                            cancellation.check()?;
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

fn rebuild_access_reports(spec: &RebuildSpec) -> Result<DiagnosticsAccessReports> {
    rebuild_access_reports_checked(spec, || Ok(()))
}

fn rebuild_access_reports_checked(
    spec: &RebuildSpec,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<DiagnosticsAccessReports> {
    let mut entries = vec![
        DiagnosticsAccessReportEntry::new_checked(
            spec.root.clone(),
            AccessIntent::Index,
            "index rebuild root",
            &mut check_control,
        )?,
        DiagnosticsAccessReportEntry::new_checked(
            write_probe_path(&spec.records_path)?.to_path_buf(),
            AccessIntent::Write,
            "index rebuild records",
            &mut check_control,
        )?,
    ];
    if let Some(content_path) = &spec.content_path {
        check_control()?;
        entries.push(DiagnosticsAccessReportEntry::new_checked(
            write_probe_path(content_path)?.to_path_buf(),
            AccessIntent::Write,
            "index rebuild content",
            &mut check_control,
        )?);
    }
    check_control()?;
    Ok(DiagnosticsAccessReports::new(entries))
}

#[cfg(test)]
fn retain_rebuild_access_checked(
    spec: &RebuildSpec,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    rebuild_access_reports_checked(spec, &mut check_control)?.access_checked(&mut check_control)
}

fn run_recovery_plan(spec: PersistentIndexRecoverySpec) -> Result<PersistentIndexPlan> {
    const WORKER: &str = "persistent index repair plan";
    let access_report =
        DiagnosticsAccessReport::new_checked(spec.root.clone(), AccessIntent::Index, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report
            .access_checked("persistent index repair root", || cancellation.check())?;
        cancellation.check()?;
        plan_index_recovery_cancellable(&spec, &cancellation)
    })
}

fn recovery_access_reports(spec: &PersistentIndexRecoverySpec) -> Result<DiagnosticsAccessReports> {
    recovery_access_reports_checked(spec, || Ok(()))
}

fn recovery_access_reports_checked(
    spec: &PersistentIndexRecoverySpec,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<DiagnosticsAccessReports> {
    Ok(DiagnosticsAccessReports::new(vec![
        DiagnosticsAccessReportEntry::new_checked(
            spec.root.clone(),
            AccessIntent::Index,
            "persistent index repair root",
            &mut check_control,
        )?,
        DiagnosticsAccessReportEntry::new_checked(
            write_probe_path(&spec.records_path)?.to_path_buf(),
            AccessIntent::Write,
            "persistent index repair records",
            &mut check_control,
        )?,
        DiagnosticsAccessReportEntry::new_checked(
            write_probe_path(&spec.state_path)?.to_path_buf(),
            AccessIntent::Write,
            "persistent index repair state",
            &mut check_control,
        )?,
        DiagnosticsAccessReportEntry::new_checked(
            write_probe_path(&spec.quarantine_dir)?.to_path_buf(),
            AccessIntent::Write,
            "persistent index repair quarantine",
            &mut check_control,
        )?,
    ]))
}

#[cfg(test)]
fn retain_recovery_access_checked(
    spec: &PersistentIndexRecoverySpec,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    recovery_access_reports_checked(spec, &mut check_control)?.access_checked(&mut check_control)
}

#[derive(Clone)]
struct DiagnosticsAccessReport {
    path: PathBuf,
    intent: AccessIntent,
    volume_report: VolumeDiscoveryReport,
}

impl DiagnosticsAccessReport {
    fn new_checked(
        path: PathBuf,
        intent: AccessIntent,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let volume_report =
            VolumeDiscoveryReport::for_containing_path_checked(&path, &mut check_control)?;
        check_control()?;
        Ok(Self {
            path,
            intent,
            volume_report,
        })
    }

    fn preflight_volume(&self, worker: &str) -> Result<()> {
        preflight_volume_access_scope_with_report(
            &self.path,
            self.intent,
            worker,
            &self.volume_report,
        )
    }

    fn access_checked(
        &self,
        worker: &str,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<ScopedAccessGuard> {
        preflight_access_scope_checked_with_volume_report(
            &self.path,
            self.intent,
            worker,
            &self.volume_report,
            check_control,
        )
    }

    fn volume(&self) -> Option<VolumeId> {
        self.volume_report
            .volume_for_path(&self.path)
            .map(|volume| volume.id)
    }
}

#[derive(Clone)]
struct DiagnosticsAccessReportEntry {
    worker: &'static str,
    report: DiagnosticsAccessReport,
}

impl DiagnosticsAccessReportEntry {
    fn new_checked(
        path: PathBuf,
        intent: AccessIntent,
        worker: &'static str,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        Ok(Self {
            worker,
            report: DiagnosticsAccessReport::new_checked(path, intent, check_control)?,
        })
    }
}

#[derive(Clone)]
struct DiagnosticsAccessReports {
    entries: Vec<DiagnosticsAccessReportEntry>,
}

impl DiagnosticsAccessReports {
    fn new(entries: Vec<DiagnosticsAccessReportEntry>) -> Self {
        Self { entries }
    }

    fn preflight_volumes(&self) -> Result<()> {
        for entry in &self.entries {
            entry.report.preflight_volume(entry.worker)?;
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
            guards.push(
                entry
                    .report
                    .access_checked(entry.worker, &mut check_control)?,
            );
        }
        check_control()?;
        Ok(guards)
    }

    fn first_volume(&self) -> Option<VolumeId> {
        self.entries.iter().find_map(|entry| entry.report.volume())
    }
}

fn run_trace_export(output: PathBuf) -> Result<String> {
    const WORKER: &str = "diagnostics trace export";
    let output_probe = write_probe_path(&output)?.to_path_buf();
    let access_report =
        DiagnosticsAccessReport::new_checked(output_probe, AccessIntent::Write, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        let report = export_operator_trace_checked(output, || cancellation.check())?;
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
    let access_reports = DiagnosticsAccessReports::new(vec![
        DiagnosticsAccessReportEntry::new_checked(
            config_write_probe_path(store.path())?.to_path_buf(),
            AccessIntent::Write,
            "diagnostics parity config",
            || Ok(()),
        )?,
        DiagnosticsAccessReportEntry::new_checked(
            existing_read_probe_path(&baseline)?.to_path_buf(),
            AccessIntent::Read,
            "diagnostics parity baseline",
            || Ok(()),
        )?,
    ]);
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "diagnostics parity baseline",
        move |cancellation| {
            cancellation.check()?;
            let _access = access_reports.access_checked(|| cancellation.check())?;
            cancellation.check()?;
            let report = select_parity_baseline_checked(&store, baseline, macos_build, || {
                cancellation.check()
            })?;
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
    let access_report =
        DiagnosticsAccessReport::new_checked(storage.clone(), AccessIntent::Read, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(WORKER, || cancellation.check())?;
        cancellation.check()?;
        match inspect_storage_checked(storage, || cancellation.check())? {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn rebuild_access_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-diagnostics-rebuild-access-pre-cancel");
        let spec = RebuildSpec::records(root.clone(), root.join("records.gfmidx"));

        let result = retain_rebuild_access_checked(&spec, || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!spec.records_path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_access_checked_can_cancel_before_state_probe() {
        let root = unique_temp_dir("gfm-diagnostics-recovery-access-cancel");
        let spec = PersistentIndexRecoverySpec::new(
            root.clone(),
            root.join("records.gfmidx"),
            root.join("state.tsv"),
            root.join("quarantine"),
        );
        let mut checks = 0usize;

        let result = retain_recovery_access_checked(&spec, || {
            checks += 1;
            if checks >= 3 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 3);
        assert!(!spec.state_path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
