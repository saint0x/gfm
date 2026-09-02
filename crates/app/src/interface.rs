use crate::volume::{resolve_volume_event_path, volume_event_invalidation_for_descriptor};
use gfm_fs::{
    read_directory_checked, scan_tree_checked, FinderMetadataReport, PackageTraversalMode,
    PackageTraversalReport, ScanOptions,
};
use gfm_index::Indexer;
use gfm_jobs::{
    JobId, JobPayloadCatalog, JobPayloadKind, JobPayloadRecord, JobProgressSnapshot,
    JobProgressState, JobProgressStore, Priority,
};
use gfm_mac::{
    current_permission_onboarding_checked, AccessIntent, AccessProbeState, CloudCommandState,
    CloudStorageState, FileProviderConflictReport, FileProviderInvalidationReport,
    FileProviderObservedInvalidation, FileProviderStateReport, FileProviderStateSnapshot,
    MountState, PermissionOnboardingPlan, SecurityAccessMode, SecurityDecisionAction,
    SecurityWorkerAction, SecurityWorkerAdmissionReport, VolumeDescriptor, VolumeDiscoveryReport,
    VolumeEventKind, VolumeEventState, VolumeKind,
};
use gfm_ops::{ConflictPolicy, Operation, OperationConflictReport};
use gfm_types::{DirectoryPage, FileEvent, FileEventKind, FileKind, GfmError, Result, VolumeId};
use gfm_ui::{
    AppLaunchSpec, ColumnSource, ColumnViewContract, ColumnViewOptions, ContextMenuContract,
    ContextMenuInput, ContextSurface, DialogContract, DialogSurface, GalleryViewContract,
    GalleryViewOptions, IconViewContract, IconViewOptions, ListViewContract, ListViewOptions,
    MenuContract, OperationConflictContract, OperationConflictInput, OperationConflictPaths,
    OperationProgressContract, OperationProgressInput, OperationProgressPayloadKind,
    OperationProgressState, PermissionAccessContract, PermissionOnboardingContract,
    PermissionOnboardingScopeContract, PermissionPromptKind, PermissionRefreshChangeContract,
    PermissionRefreshContract, ProviderConflictContract, ProviderConflictInput, SearchResultsBatch,
    SearchResultsContract, SearchResultsOptions, SearchResultsStage, SidebarCloudInvalidation,
    SidebarCloudState, SidebarContract, SidebarPathSnapshot, SidebarPathState,
    SidebarVolumeEventKind, SidebarVolumeInvalidation, SidebarVolumeKind, SidebarVolumeMountState,
    SidebarVolumeSpec, TitlebarContract, ToolbarContract, TrashEntryMetadata, TrashViewContract,
    TrashViewOptions, VirtualSurface, VirtualizationContract, WindowLifecycleContract,
    WindowSessionContract, WindowSessionStore,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "app" => {
            let spec = app_launch_spec(args.next())?;
            gfm_ui::run_native(spec)?;
        }
        "ui-contract" => {
            let spec = app_launch_spec(args.next())?;
            println!("{}", WindowLifecycleContract::from_spec(&spec)?.as_tsv());
        }
        "ui-menu-contract" => {
            println!("{}", MenuContract::finder_default().as_tsv());
        }
        "ui-context-menu-contract" => {
            let surface = args
                .next()
                .unwrap_or_else(|| "file".to_string())
                .parse::<ContextSurface>()
                .map_err(GfmError::Format)?;
            let selection_count = args
                .next()
                .map(|value| parse_u16(&value, "selection-count"))
                .transpose()?
                .unwrap_or(match surface {
                    ContextSurface::Empty => 0,
                    _ => 1,
                });
            let writable = args
                .next()
                .map(|value| parse_bool(&value, "writable"))
                .transpose()?
                .unwrap_or(true);
            let ejectable = args
                .next()
                .map(|value| parse_bool(&value, "ejectable"))
                .transpose()?
                .unwrap_or(surface == ContextSurface::Volume);
            let has_clipboard_items = args
                .next()
                .map(|value| parse_bool(&value, "has-clipboard-items"))
                .transpose()?
                .unwrap_or(true);

            let input = ContextMenuInput::new(surface)
                .with_selection_count(selection_count)
                .with_writable(writable)
                .with_ejectable(ejectable)
                .with_clipboard_items(has_clipboard_items);
            println!("{}", ContextMenuContract::finder_default(input).as_tsv());
        }
        "ui-dialog-contract" => {
            let surface = args
                .next()
                .unwrap_or_else(|| "alert".to_string())
                .parse::<DialogSurface>()
                .map_err(GfmError::Format)?;
            let contract = if surface == DialogSurface::Progress {
                let paused = match args.next().as_deref() {
                    Some("paused") => true,
                    Some("running") | None => false,
                    Some(other) => {
                        return Err(GfmError::Format(format!(
                            "progress dialog state must be running or paused; got `{other}`"
                        )));
                    }
                };
                let cancellable = args
                    .next()
                    .map(|value| parse_bool(&value, "progress cancellable"))
                    .transpose()?
                    .unwrap_or(true);
                DialogContract::operation_progress(paused, cancellable)
            } else {
                DialogContract::finder_default(surface)
            };
            println!("{}", contract.as_tsv());
        }
        "ui-permission-onboarding-contract" => {
            let (plan, refresh) = permission_onboarding_contract_inputs_checked(|| Ok(()))?;
            print_permission_onboarding_contract(
                plan,
                refresh.as_ref().map(permission_refresh_contract),
            );
        }
        "ui-permission-access-contract" => {
            let path = required_path(args.next(), "ui-permission-access-contract requires a path")?;
            let intent = AccessIntent::parse(&required_string(
                args.next(),
                "ui-permission-access-contract requires an access intent",
            )?)?;
            let worker = args
                .next()
                .unwrap_or_else(|| "just-in-time permission".to_string());
            let refresh = crate::permission_refresh::refresh_permission_state(
                crate::permission_refresh::PermissionRefreshAudience::Ui,
                "permission-access",
            )?;
            let admission = crate::access::worker_admission_with_volume_gate_checked(
                &path,
                intent,
                worker,
                || Ok(()),
            )?;
            print_permission_access_contract(
                &admission,
                refresh.as_ref().map(permission_refresh_contract),
            )?;
        }
        "ui-permission-refresh-compare-contract" => {
            let previous_path = required_path(
                args.next(),
                "ui-permission-refresh-compare-contract requires a previous state path",
            )?;
            let current_path = required_path(
                args.next(),
                "ui-permission-refresh-compare-contract requires a current state path",
            )?;
            let previous_access_report = InterfaceAccessReport::new_checked(
                previous_path.clone(),
                AccessIntent::Read,
                || Ok(()),
            )?;
            let current_access_report = InterfaceAccessReport::new_checked(
                current_path.clone(),
                AccessIntent::Read,
                || Ok(()),
            )?;
            previous_access_report.preflight_volume("ui permission refresh previous state")?;
            current_access_report.preflight_volume("ui permission refresh current state")?;
            let _previous_access = previous_access_report
                .access_checked("ui permission refresh previous state", || Ok(()))?;
            let _current_access = current_access_report
                .access_checked("ui permission refresh current state", || Ok(()))?;
            let previous = gfm_mac::PermissionStateSnapshot::read(&previous_path)?;
            let current = gfm_mac::PermissionStateSnapshot::read(&current_path)?;
            let refresh =
                gfm_mac::PermissionStateInvalidationReport::evaluate(Some(&previous), &current);
            println!("{}", permission_refresh_contract(&refresh).as_tsv());
        }
        "ui-progress-job-contract" => {
            let path = required_path(
                args.next(),
                "ui-progress-job-contract requires a progress path",
            )?;
            let job_id = JobId::from_raw(parse_u64_arg(
                args.next(),
                "ui-progress-job-contract requires a job id",
            )?);
            let snapshots = read_ui_progress_snapshots(&path)?;
            let snapshot = snapshots
                .iter()
                .find(|snapshot| snapshot.id == job_id)
                .ok_or_else(|| {
                    GfmError::Format(format!(
                        "progress store {} does not contain job {}",
                        path.display(),
                        job_id.value()
                    ))
                })?;
            println!("{}", operation_progress_contract(snapshot, None).as_tsv());
        }
        "ui-fileprovider-conflict-contract" => {
            let path = required_path(
                args.next(),
                "ui-fileprovider-conflict-contract requires a FileProvider path",
            )?;
            let report = read_ui_fileprovider_conflict(path)?;
            let contract = ProviderConflictContract::from_input(ProviderConflictInput::new(
                report.path.display().to_string(),
                report.has_unresolved_conflict,
                report
                    .affected_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect(),
                report.reveal_command == CloudCommandState::Enabled,
                report.block_operations,
                report.reason,
            ));
            println!("{}", contract.as_tsv());
        }
        "ui-operation-conflict-contract" => {
            let operation = parse_conflict_contract_operation(args)?;
            let conflict = parse_optional_conflict_policy(args.next().as_deref())?;
            let report = OperationConflictReport::evaluate(&operation, conflict)?;
            println!("{}", operation_conflict_contract(&report).as_tsv());
        }
        "ui-operation-conflict-resolve" => {
            let store_path = required_path(
                args.next(),
                "ui-operation-conflict-resolve requires an operation conflict store path",
            )?;
            let target = required_string(
                args.next(),
                "ui-operation-conflict-resolve requires a target path",
            )?;
            let policy = parse_required_resolution_policy(args.next())?;
            let (resolved, store_path) = resolve_ui_operation_conflict(store_path, target, policy)?;
            println!(
                "operation-conflict-control\tresolve\ttarget={}\tpolicy={}\tblocks-operation={}\treason={}",
                escape_interface_field(&resolved.target),
                escape_interface_field(&resolved.selected_policy),
                resolved.blocks_operation,
                escape_interface_field(&resolved.reason)
            );
            println!(
                "{}",
                runtime_operation_conflict_contract(&resolved, Some(&store_path)).as_tsv()
            );
        }
        "ui-titlebar-contract" => {
            let spec = app_launch_spec(args.next())?;
            println!("{}", TitlebarContract::from_spec(&spec)?.as_tsv());
        }
        "ui-session-contract" => {
            let spec = app_launch_spec(args.next())?;
            let store = args
                .next()
                .map(WindowSessionStore::new)
                .unwrap_or_else(WindowSessionStore::platform_default);
            println!(
                "{}",
                WindowSessionContract::from_spec(&spec, &store, 0).as_tsv()
            );
        }
        "ui-toolbar-contract" => {
            let path = default_current_path(args.next());
            println!("{}", ToolbarContract::finder_default(path).as_tsv());
        }
        "ui-sidebar-contract" => {
            let path = default_current_path(args.next());
            println!(
                "{}",
                SidebarContract::from_path_snapshot(
                    path,
                    SidebarPathSnapshot::discover(),
                    native_sidebar_volumes_checked(|| Ok(()))?
                )
                .as_tsv()
            );
        }
        "ui-sidebar-fileprovider-contract" => {
            let current_path = default_current_path(args.next());
            let provider_path = required_path(
                args.next(),
                "ui-sidebar-fileprovider-contract requires a FileProvider path",
            )?;
            let report = read_ui_fileprovider_sidebar_state(provider_path.clone())?;
            println!(
                "{}",
                SidebarContract::from_path_snapshot_with_icloud_progress(
                    current_path,
                    SidebarPathSnapshot::discover()
                        .with_icloud_drive(provider_path, SidebarPathState::Available),
                    sidebar_cloud_state(report.storage_state),
                    report.progress.percent_milli,
                    Some(report.progress.source.to_string()),
                    report.progress.reason,
                    Vec::new(),
                )
                .as_tsv()
            );
        }
        "ui-sidebar-fileprovider-invalidation" => {
            let previous = CloudStorageState::parse(&required_string(
                args.next(),
                "ui-sidebar-fileprovider-invalidation requires a previous FileProvider state",
            )?)?;
            let provider_path = required_path(
                args.next(),
                "ui-sidebar-fileprovider-invalidation requires a FileProvider path",
            )?;
            let report = read_ui_fileprovider_sidebar_invalidation(provider_path, previous)?;
            println!(
                "{}",
                SidebarCloudInvalidation::new(
                    report.path,
                    sidebar_cloud_state(report.previous),
                    sidebar_cloud_state(report.current.storage_state),
                    report.current.progress.percent_milli,
                    report.invalidate_sidebar,
                    report.reason,
                )
                .with_progress_context(
                    Some(report.current.progress.source.to_string()),
                    report.current.progress.reason,
                )
                .as_tsv()
            );
        }
        "ui-sidebar-fileprovider-observed-invalidation" => {
            let state_path = required_path(
                args.next(),
                "ui-sidebar-fileprovider-observed-invalidation requires a state path",
            )?;
            let event_kind = required_string(
                args.next(),
                "ui-sidebar-fileprovider-observed-invalidation requires an event kind",
            )?;
            let path = required_path(
                args.next(),
                "ui-sidebar-fileprovider-observed-invalidation requires a path",
            )?;
            let event =
                parse_fileprovider_event(&event_kind, path, args.next().map(PathBuf::from))?;
            let observed = run_ui_fileprovider_observed_invalidation(state_path, event)?;
            println!("{}", observed_sidebar_invalidation_tsv(&observed));
        }
        "ui-sidebar-fileprovider-observer-probe" => {
            let state_path = required_path(
                args.next(),
                "ui-sidebar-fileprovider-observer-probe requires a state path",
            )?;
            let root = required_path(
                args.next(),
                "ui-sidebar-fileprovider-observer-probe requires a root",
            )?;
            let target = required_path(
                args.next(),
                "ui-sidebar-fileprovider-observer-probe requires a FileProvider target path",
            )?;
            let observed = crate::platform::run_fileprovider_observer_probe(
                &state_path,
                &root,
                &target,
                "ui fileprovider sidebar observer",
            )?;
            println!("{}", observed_sidebar_invalidation_tsv(&observed));
        }
        "ui-sidebar-volume-invalidation" => {
            let kind = parse_volume_event_kind(&required_string(
                args.next(),
                "ui-sidebar-volume-invalidation requires a volume event kind",
            )?)?;
            let resolution = resolve_volume_event_path(kind, args.next().map(PathBuf::from))?;
            let platform = resolution.invalidation_report(kind);
            let previous = (kind == VolumeEventKind::Disappeared)
                .then(|| resolution.descriptor.as_ref().map(sidebar_volume_spec))
                .flatten();
            let current = (kind != VolumeEventKind::Disappeared)
                .then(|| resolution.descriptor.as_ref().map(sidebar_volume_spec))
                .flatten();
            let invalidation = SidebarVolumeInvalidation::from_event(
                sidebar_volume_event_kind(kind),
                resolution.path.clone(),
                previous.as_ref(),
                current.as_ref(),
                platform.invalidate_sidebar,
                platform.reason.clone(),
            );
            println!(
                "{}",
                invalidation
                    .with_platform_statuses(
                        volume_status_string(platform.previous_native_status),
                        volume_status_string(platform.previous_resource_status),
                        volume_status_string(platform.previous_mount_table_status),
                        volume_status_string(platform.current_native_status),
                        volume_status_string(platform.current_resource_status),
                        volume_status_string(platform.current_mount_table_status),
                    )
                    .as_tsv()
            );
        }
        "ui-sidebar-volume-api-status-invalidation" => {
            let mut descriptor = VolumeDescriptor::for_path("/")?;
            descriptor.stable_identity = "diskarbitration:uuid:UI-API-DESCRIPTION".to_string();
            descriptor.label = "UI API Description".to_string();
            descriptor.path = PathBuf::from("/Volumes/UI API Description");
            descriptor.kind = VolumeKind::External;
            descriptor.case_sensitive = Some(true);
            descriptor.case_preserving = Some(true);
            descriptor.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
            descriptor.native_reason =
                Some("DiskArbitration unavailable during refresh".to_string());
            descriptor.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
            descriptor.resource_reason =
                Some("URL resource values unavailable during refresh".to_string());
            descriptor.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
            descriptor.mount_table_reason =
                Some("mount table unavailable during refresh".to_string());
            let platform = volume_event_invalidation_for_descriptor(
                VolumeEventKind::DescriptionChanged,
                descriptor.path.clone(),
                &descriptor,
            );
            let current = sidebar_volume_spec(&descriptor);
            let invalidation = SidebarVolumeInvalidation::from_event(
                SidebarVolumeEventKind::DescriptionChanged,
                platform.path.clone(),
                None,
                Some(&current),
                platform.invalidate_sidebar,
                platform.reason.clone(),
            )
            .with_platform_statuses(
                volume_status_string(platform.previous_native_status),
                volume_status_string(platform.previous_resource_status),
                volume_status_string(platform.previous_mount_table_status),
                volume_status_string(platform.current_native_status),
                volume_status_string(platform.current_resource_status),
                volume_status_string(platform.current_mount_table_status),
            );
            println!("{}", invalidation.as_tsv());
        }
        "ui-sidebar-volume-state-invalidation" => {
            let mut previous_paths = Vec::new();
            loop {
                let arg = required_string(
                    args.next(),
                    "ui-sidebar-volume-state-invalidation requires previous paths, `--`, event kind, and optional event path",
                )?;
                if arg == "--" {
                    break;
                }
                previous_paths.push(PathBuf::from(arg));
            }
            let kind = parse_volume_event_kind(&required_string(
                args.next(),
                "ui-sidebar-volume-state-invalidation requires an event kind after `--`",
            )?)?;
            let resolution = resolve_volume_event_path(kind, args.next().map(PathBuf::from))?;
            let mut state =
                VolumeEventState::new(VolumeDiscoveryReport::from_paths_checked(previous_paths)?);
            let current = (kind != VolumeEventKind::Disappeared)
                .then_some(resolution.descriptor)
                .flatten();
            let transition = state.apply_parts_transition(
                kind,
                resolution.native_status,
                resolution.path.clone(),
                current,
                resolution.native_reason,
            );
            let previous = transition.previous.as_ref().map(sidebar_volume_spec);
            let current = transition.current.as_ref().map(sidebar_volume_spec);
            let platform = transition.invalidation;
            let invalidation = SidebarVolumeInvalidation::from_event(
                sidebar_volume_event_kind(kind),
                platform.path.clone(),
                previous.as_ref(),
                current.as_ref(),
                platform.invalidate_sidebar,
                platform.reason.clone(),
            );
            println!(
                "{}",
                invalidation
                    .with_platform_statuses(
                        volume_status_string(platform.previous_native_status),
                        volume_status_string(platform.previous_resource_status),
                        volume_status_string(platform.previous_mount_table_status),
                        volume_status_string(platform.current_native_status),
                        volume_status_string(platform.current_resource_status),
                        volume_status_string(platform.current_mount_table_status),
                    )
                    .as_tsv()
            );
        }
        "ui-icon-view-contract" => {
            let path = required_path(
                args.next(),
                "ui-icon-view-contract requires a directory path",
            )?;
            let columns = optional_u16(args.next(), "columns", 6)?;
            let viewport_rows = optional_u16(args.next(), "viewport-rows", 4)?;
            let scroll_row = optional_u16(args.next(), "scroll-row", 0)?;
            let page = read_directory_with_access(&path, "ui icon view")?;
            let options = IconViewOptions::default()
                .with_columns(columns)
                .with_viewport_rows(viewport_rows)
                .with_scroll_row(scroll_row);
            println!(
                "{}",
                IconViewContract::from_records(&page.entries, options).as_tsv()
            );
        }
        "ui-list-view-contract" => {
            let path = required_path(
                args.next(),
                "ui-list-view-contract requires a directory path",
            )?;
            let viewport_rows = optional_u16(args.next(), "viewport-rows", 24)?;
            let scroll_row = optional_u32(args.next(), "scroll-row", 0)?;
            let page = read_directory_with_access(&path, "ui list view")?;
            let options = ListViewOptions::default()
                .with_viewport_rows(viewport_rows)
                .with_scroll_row(scroll_row);
            println!(
                "{}",
                ListViewContract::from_records(&page.entries, options).as_tsv()
            );
        }
        "ui-column-view-contract" => {
            let path = required_path(
                args.next(),
                "ui-column-view-contract requires a directory path",
            )?;
            let viewport_rows = optional_u16(args.next(), "viewport-rows", 24)?;
            let scroll_row = optional_u32(args.next(), "scroll-row", 0)?;
            let selected_name = args.next();
            let page = read_directory_with_access(&path, "ui column view")?;
            let selected_record = selected_name
                .as_deref()
                .and_then(|name| page.entries.iter().find(|record| record.name == name));
            let mut sources = vec![ColumnSource::new(path.clone(), page.entries.clone())
                .with_scroll_row(scroll_row)
                .with_selected(selected_record.map(|record| record.id))];
            if let Some(record) =
                selected_record.filter(|record| record.kind == FileKind::Directory)
            {
                let child_page = read_directory_with_access(&record.path, "ui column child view")?;
                sources.push(ColumnSource::new(record.path.clone(), child_page.entries));
            }
            let options = ColumnViewOptions::default().with_viewport_rows(viewport_rows);
            println!(
                "{}",
                ColumnViewContract::from_sources(sources, options).as_tsv()
            );
        }
        "ui-gallery-view-contract" => {
            let path = required_path(
                args.next(),
                "ui-gallery-view-contract requires a directory path",
            )?;
            let viewport_items = optional_u16(args.next(), "viewport-items", 8)?;
            let scroll_item = optional_u32(args.next(), "scroll-item", 0)?;
            let selected_name = args.next();
            let page = read_directory_with_access(&path, "ui gallery view")?;
            let selected = selected_name
                .as_deref()
                .and_then(|name| page.entries.iter().find(|record| record.name == name))
                .map(|record| record.id);
            let options = GalleryViewOptions::default()
                .with_viewport_items(viewport_items)
                .with_scroll_item(scroll_item)
                .with_selected(selected);
            println!(
                "{}",
                GalleryViewContract::from_records(&page.entries, options).as_tsv()
            );
        }
        "ui-search-results-contract" => {
            let root = required_path(
                args.next(),
                "ui-search-results-contract requires a root path",
            )?;
            let query = required_string(
                args.next(),
                "ui-search-results-contract requires a query string",
            )?;
            let viewport_rows = optional_u16(args.next(), "viewport-rows", 24)?;
            let scroll_row = optional_u32(args.next(), "scroll-row", 0)?;
            let access_report =
                InterfaceAccessReport::new_checked(root.clone(), AccessIntent::Index, || Ok(()))?;
            access_report.preflight_volume("ui search")?;
            eprintln!(
                "{}",
                access_report.as_tsv("ui-search-volume-access", "ui search")
            );
            let volume = access_report.volume();
            let query_for_worker = query.clone();
            let batches = crate::runtime::run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "ui search",
                move |cancellation| {
                    cancellation.check()?;
                    let _access =
                        access_report.access_checked("ui search", || cancellation.check())?;
                    cancellation.check()?;
                    let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
                    let session = snapshot.query_session();
                    let batches = session
                        .stream_search(&query_for_worker, 50)?
                        .into_iter()
                        .map(|batch| {
                            SearchResultsBatch::new(search_results_stage(batch.stage), batch.hits)
                        })
                        .collect();
                    Ok(batches)
                },
            )?;
            let options = SearchResultsOptions::new(query)
                .with_viewport_rows(viewport_rows)
                .with_scroll_row(scroll_row);
            println!(
                "{}",
                SearchResultsContract::from_batches(batches, options).as_tsv()
            );
        }
        "ui-trash-view-contract" => {
            let path = required_path(
                args.next(),
                "ui-trash-view-contract requires a trash directory path",
            )?;
            let metadata_path = args
                .next()
                .and_then(|value| (value != "-").then(|| PathBuf::from(value)));
            let viewport_rows = optional_u16(args.next(), "viewport-rows", 24)?;
            let scroll_row = optional_u32(args.next(), "scroll-row", 0)?;
            let page = read_directory_with_access(&path, "ui trash view")?;
            let metadata = metadata_path
                .as_ref()
                .map(|path| read_trash_restore_metadata(path.as_path()))
                .transpose()?
                .unwrap_or_default();
            let options = TrashViewOptions::default()
                .with_metadata(metadata)
                .with_viewport_rows(viewport_rows)
                .with_scroll_row(scroll_row);
            println!(
                "{}",
                TrashViewContract::from_records(&page.entries, options).as_tsv()
            );
        }
        "package-traversal" => {
            let root = required_path(args.next(), "package-traversal requires a root path")?;
            let mode = parse_package_traversal_mode(args.next().as_deref())?;
            let options = ScanOptions::default().with_package_traversal(mode);
            let access_report =
                InterfaceAccessReport::new_checked(root.clone(), AccessIntent::Read, || Ok(()))?;
            access_report.preflight_volume("package traversal")?;
            let volume = access_report.volume();
            let options_for_worker = options.clone();
            let page = crate::runtime::run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "package traversal",
                move |cancellation| {
                    cancellation.check()?;
                    let _access = access_report
                        .access_checked("package traversal", || cancellation.check())?;
                    cancellation.check()?;
                    scan_tree_checked(&root, options_for_worker, || cancellation.check())
                },
            )?;
            let report = PackageTraversalReport::from_page(&page, &options.package_policy);
            println!("{}", report.as_tsv());
        }
        "finder-metadata" => {
            let path = required_path(args.next(), "finder-metadata requires a path")?;
            println!("{}", read_finder_metadata(path)?.as_tsv());
        }
        "ui-virtualization-contract" => {
            let surface = parse_virtual_surface(args.next().as_deref())?;
            let total = parse_usize_arg(
                args.next(),
                "ui-virtualization-contract requires a total row/item count",
            )?;
            let viewport = parse_u16_arg(
                args.next(),
                "ui-virtualization-contract requires a viewport row/item count",
            )?;
            let scroll = parse_u32_arg(
                args.next(),
                "ui-virtualization-contract requires a scroll row/item",
            )?;
            let contract = if surface == VirtualSurface::IconGrid {
                let columns = optional_u16(args.next(), "columns", 6)?;
                VirtualizationContract::grid(
                    total,
                    scroll.min(u32::from(u16::MAX)) as u16,
                    viewport,
                    columns,
                )
            } else if surface == VirtualSurface::GalleryFilmstrip {
                VirtualizationContract::items(surface, total, scroll, viewport)
            } else {
                VirtualizationContract::rows(surface, total, scroll, viewport)
            };
            println!("{}", contract.as_tsv());
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn print_permission_onboarding_contract(
    plan: gfm_mac::PermissionOnboardingPlan,
    refresh: Option<PermissionRefreshContract>,
) {
    let onboarding = permission_onboarding_contract(&plan);
    println!(
        "{}",
        DialogContract::permission_prompt(onboarding.prompt_kind).as_tsv()
    );
    println!("{}", onboarding.as_tsv());
    if let Some(refresh) = refresh {
        println!("{}", refresh.as_tsv());
    }
}

fn print_permission_access_contract(
    admission: &SecurityWorkerAdmissionReport,
    refresh: Option<PermissionRefreshContract>,
) -> Result<()> {
    let access = permission_access_contract(admission);
    let dialog = validated_permission_access_dialog(&access)?;
    println!("{}", dialog.as_tsv());
    if let Some(refresh) = refresh {
        println!("{}", refresh.as_tsv());
    }
    println!("{}", access.as_tsv());
    println!("{}", admission.as_tsv());
    Ok(())
}

fn validated_permission_access_dialog(access: &PermissionAccessContract) -> Result<DialogContract> {
    let spec = AppLaunchSpec::new(&access.path).with_permission_access(access.clone());
    spec.validate()?;
    Ok(DialogContract::permission_prompt_for_action(
        access.prompt_kind,
        &access.prompt_action,
    ))
}

fn permission_access_contract(
    admission: &SecurityWorkerAdmissionReport,
) -> PermissionAccessContract {
    let prompt_action = permission_prompt_action_for_admission(admission);
    let (promptable, prompt_source) = permission_prompt_orchestration_for_admission(admission);
    PermissionAccessContract {
        path: admission.access.path.display().to_string(),
        intent: admission.access.intent.as_str().to_string(),
        scope: admission.access.scope.as_str().to_string(),
        probe: admission.access.probe.as_str().to_string(),
        mode: admission.access.mode.as_str().to_string(),
        access_action: admission.access.action.as_str().to_string(),
        worker_action: admission.worker_action.as_str().to_string(),
        can_touch_filesystem: admission.can_touch_filesystem,
        bookmark_required: false,
        bookmark_access: false,
        refresh_on_permission_change: false,
        prompt_kind: permission_prompt_kind_for_admission(admission),
        prompt_action: prompt_action.to_string(),
        promptable,
        prompt_source: prompt_source.to_string(),
        reason: admission.reason.clone(),
    }
    .with_bookmark_state(
        admission.access.bookmark_required,
        admission.needs_bookmark_access,
    )
    .with_refresh_on_permission_change(admission.refresh_on_permission_change)
}

fn parse_conflict_contract_operation(args: &mut impl Iterator<Item = String>) -> Result<Operation> {
    let kind = required_string(
        args.next(),
        "ui-operation-conflict-contract requires copy, move, rename, or restore",
    )?;
    let from = required_path(
        args.next(),
        "ui-operation-conflict-contract requires a source path",
    )?;
    let to = required_path(
        args.next(),
        "ui-operation-conflict-contract requires a target path",
    )?;
    match kind.as_str() {
        "copy" => Ok(Operation::Copy { from, to }),
        "move" => Ok(Operation::Move { from, to }),
        "rename" => Ok(Operation::Rename { from, to }),
        "restore" => Ok(Operation::Restore { from, to }),
        other => Err(GfmError::Format(format!(
            "ui-operation-conflict-contract operation must be copy, move, rename, or restore; got `{other}`"
        ))),
    }
}

fn parse_optional_conflict_policy(value: Option<&str>) -> Result<ConflictPolicy> {
    match value.unwrap_or("fail") {
        "fail" => Ok(ConflictPolicy::Fail),
        "replace" => Ok(ConflictPolicy::Replace),
        "keep-both" => Ok(ConflictPolicy::KeepBoth),
        "merge" => Ok(ConflictPolicy::Merge),
        "skip" => Ok(ConflictPolicy::Skip),
        other => Err(GfmError::Format(format!(
            "conflict policy must be fail, replace, keep-both, merge, or skip; got `{other}`"
        ))),
    }
}

fn parse_required_resolution_policy(value: Option<String>) -> Result<ConflictPolicy> {
    let value = required_string(
        value,
        "ui-operation-conflict-resolve requires replace, keep-both, merge, or skip",
    )?;
    let policy = parse_optional_conflict_policy(Some(&value))?;
    if policy == ConflictPolicy::Fail {
        return Err(GfmError::Format(
            "ui-operation-conflict-resolve requires replace, keep-both, merge, or skip".to_string(),
        ));
    }
    Ok(policy)
}

fn operation_conflict_contract(report: &OperationConflictReport) -> OperationConflictContract {
    OperationConflictContract::from_input(OperationConflictInput::new(
        report.operation,
        OperationConflictPaths::new(
            report
                .source
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            report
                .target
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        report.target_kind.as_str(),
        report.selected_policy.as_str(),
        report
            .available_policies
            .iter()
            .map(|policy| policy.as_str().to_string())
            .collect(),
        report.blocks_operation,
        report.reason.clone(),
    ))
}

fn runtime_operation_conflict_contract(
    conflict: &crate::runtime::RuntimeOperationConflict,
    store_path: Option<&std::path::Path>,
) -> OperationConflictContract {
    OperationConflictContract::from_input(runtime_operation_conflict_input(conflict, store_path))
}

fn runtime_operation_conflict_input(
    conflict: &crate::runtime::RuntimeOperationConflict,
    store_path: Option<&std::path::Path>,
) -> OperationConflictInput {
    let input = OperationConflictInput::new(
        conflict.operation.clone(),
        OperationConflictPaths::new(conflict.source.clone(), conflict.target.clone()),
        conflict.target_kind.clone(),
        conflict.selected_policy.clone(),
        conflict.available_policies.clone(),
        conflict.blocks_operation,
        conflict.reason.clone(),
    );
    if let Some(path) = store_path {
        input.with_store_path(path.display().to_string())
    } else {
        input
    }
}

fn native_sidebar_volumes_checked(
    check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<SidebarVolumeSpec>> {
    Ok(VolumeDiscoveryReport::discover_checked(check_control)?
        .volumes
        .iter()
        .filter(|volume| volume.kind != VolumeKind::System)
        .map(sidebar_volume_spec)
        .collect())
}

fn sidebar_volume_spec(volume: &VolumeDescriptor) -> SidebarVolumeSpec {
    SidebarVolumeSpec::from_native_seed(
        &volume.stable_identity,
        volume.label.clone(),
        volume.path.clone(),
        volume.ejectable,
    )
    .with_volume_state(
        sidebar_volume_kind(volume.kind),
        sidebar_volume_mount_state(volume.mount_state),
        volume.read_only,
        volume.network,
        volume.reachable,
    )
    .with_volume_media_state(
        volume.removable,
        volume.case_sensitive,
        volume.case_preserving,
    )
    .with_platform_api_context(
        volume_status_string(volume.native_status),
        volume.native_reason.clone(),
        volume_status_string(volume.resource_status),
        volume.resource_reason.clone(),
        volume_status_string(volume.mount_table_status),
        volume.mount_table_reason.clone(),
    )
}

fn sidebar_volume_kind(kind: VolumeKind) -> SidebarVolumeKind {
    match kind {
        VolumeKind::System | VolumeKind::Internal => SidebarVolumeKind::Internal,
        VolumeKind::External => SidebarVolumeKind::External,
        VolumeKind::Removable => SidebarVolumeKind::Removable,
        VolumeKind::Network => SidebarVolumeKind::Network,
        VolumeKind::DiskImage => SidebarVolumeKind::DiskImage,
        VolumeKind::Unknown => SidebarVolumeKind::Unknown,
    }
}

fn sidebar_volume_mount_state(state: MountState) -> SidebarVolumeMountState {
    match state {
        MountState::Mounted => SidebarVolumeMountState::Mounted,
        MountState::Unmounted => SidebarVolumeMountState::Unmounted,
        MountState::Stale => SidebarVolumeMountState::Stale,
    }
}

fn volume_status_string(status: Option<gfm_mac::NativeVolumeStatus>) -> Option<String> {
    status.map(|status| status.as_str().to_string())
}

fn parse_volume_event_kind(kind: &str) -> Result<VolumeEventKind> {
    match kind {
        "appeared" => Ok(VolumeEventKind::Appeared),
        "description-changed" => Ok(VolumeEventKind::DescriptionChanged),
        "disappeared" => Ok(VolumeEventKind::Disappeared),
        "unavailable" => Ok(VolumeEventKind::Unavailable),
        other => Err(GfmError::Format(format!(
            "unknown volume event kind `{other}`"
        ))),
    }
}

fn sidebar_volume_event_kind(kind: VolumeEventKind) -> SidebarVolumeEventKind {
    match kind {
        VolumeEventKind::Appeared => SidebarVolumeEventKind::Appeared,
        VolumeEventKind::DescriptionChanged => SidebarVolumeEventKind::DescriptionChanged,
        VolumeEventKind::Disappeared => SidebarVolumeEventKind::Disappeared,
        VolumeEventKind::Unavailable => SidebarVolumeEventKind::Unavailable,
    }
}

fn parse_fileprovider_event(kind: &str, path: PathBuf, to: Option<PathBuf>) -> Result<FileEvent> {
    let event_kind = match kind {
        "create" => FileEventKind::Create,
        "metadata" => FileEventKind::Metadata,
        "modify" => FileEventKind::Modify,
        "remove" => FileEventKind::Remove,
        "rescan" => FileEventKind::Rescan,
        "other" => FileEventKind::Other,
        "rename" => FileEventKind::Rename {
            from: path.clone(),
            to: to.ok_or_else(|| {
                GfmError::Format(
                    "ui-sidebar-fileprovider-observed-invalidation rename requires a destination path"
                        .to_string(),
                )
            })?,
        },
        other => {
            return Err(GfmError::Format(format!(
                "unsupported FileProvider event kind `{other}`"
            )))
        }
    };
    Ok(FileEvent::new(path, event_kind))
}

fn observed_sidebar_invalidation_tsv(observed: &FileProviderObservedInvalidation) -> String {
    let mut lines = vec![observed.as_tsv()];
    lines.extend(observed.report.changes.iter().map(|report| {
        SidebarCloudInvalidation::new(
            report.path.clone(),
            sidebar_cloud_state(report.previous),
            sidebar_cloud_state(report.current.storage_state),
            report.current.progress.percent_milli,
            report.invalidate_sidebar,
            report.reason,
        )
        .with_progress_context(
            Some(report.current.progress.source.to_string()),
            report.current.progress.reason.clone(),
        )
        .as_tsv()
    }));
    lines.join("\n")
}

#[cfg(test)]
fn retain_fileprovider_event_access_checked(
    event: &FileEvent,
    previous: Option<&FileProviderStateSnapshot>,
    worker: &'static str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<crate::access::ScopedAccessGuard>> {
    check_control()?;
    let mut guards = Vec::new();
    let paths = fileprovider_event_access_paths(event, previous, worker)?;
    for path in unique_fileprovider_paths(paths.iter().map(PathBuf::as_path)) {
        check_control()?;
        guards.push(
            InterfaceAccessReport::new_checked(
                path.to_path_buf(),
                AccessIntent::Read,
                &mut check_control,
            )?
            .access_checked(worker, &mut check_control)?,
        );
    }
    check_control()?;
    Ok(guards)
}

fn unique_fileprovider_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Vec<&'a Path> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert((*path).to_path_buf()))
        .collect()
}

fn fileprovider_event_access_paths(
    event: &FileEvent,
    previous: Option<&FileProviderStateSnapshot>,
    worker: &str,
) -> Result<Vec<PathBuf>> {
    match &event.kind {
        FileEventKind::Rename { from, to } => [from, to]
            .into_iter()
            .map(|path| fileprovider_event_access_path(path, previous, worker))
            .collect(),
        FileEventKind::Remove => Ok(vec![fileprovider_event_access_path(
            &event.path,
            previous,
            worker,
        )?]),
        FileEventKind::Create
        | FileEventKind::Metadata
        | FileEventKind::Modify
        | FileEventKind::Rescan
        | FileEventKind::Other => Ok(vec![event.path.clone()]),
    }
}

fn fileprovider_raw_event_paths(event: &FileEvent) -> Vec<PathBuf> {
    match &event.kind {
        FileEventKind::Rename { from, to } => vec![from.clone(), to.clone()],
        FileEventKind::Create
        | FileEventKind::Metadata
        | FileEventKind::Modify
        | FileEventKind::Remove
        | FileEventKind::Rescan
        | FileEventKind::Other => vec![event.path.clone()],
    }
}

fn fileprovider_event_access_path(
    path: &Path,
    previous: Option<&FileProviderStateSnapshot>,
    worker: &str,
) -> Result<PathBuf> {
    if snapshot_tracks_path_or_descendant(previous, path) {
        return write_probe_existing_ancestor(path, worker);
    }
    Ok(path.to_path_buf())
}

fn snapshot_tracks_path_or_descendant(
    previous: Option<&FileProviderStateSnapshot>,
    path: &Path,
) -> bool {
    previous.is_some_and(|snapshot| {
        snapshot
            .entries
            .iter()
            .any(|entry| entry.path == path || entry.path.starts_with(path))
    })
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Ok(crate::parent_or_cwd(path)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("interface write path metadata unavailable: {err}"),
        )),
    }
}

fn write_probe_existing_ancestor(path: &Path, worker: &str) -> Result<PathBuf> {
    preflight_write_target_volume(path, worker)?;
    let mut candidate = write_probe_path(path)?.to_path_buf();
    loop {
        match fs::metadata(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(GfmError::io(
                    &candidate,
                    format!("interface write path metadata unavailable: {err}"),
                ));
            }
        }
        let Some(parent) = candidate.parent() else {
            break;
        };
        if parent == candidate {
            break;
        }
        candidate = parent.to_path_buf();
    }
    Ok(candidate)
}

fn preflight_write_target_volume(path: &Path, worker: &str) -> Result<()> {
    let volume_path = crate::parent_or_cwd(path);
    let volume_report = VolumeDiscoveryReport::for_containing_path_checked(volume_path, || Ok(()))?;
    crate::access::preflight_volume_access_scope_with_report(
        volume_path,
        AccessIntent::Write,
        worker,
        &volume_report,
    )
}

#[derive(Clone)]
struct InterfaceAccessReport {
    path: PathBuf,
    intent: AccessIntent,
    volume_report: VolumeDiscoveryReport,
}

impl InterfaceAccessReport {
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
        crate::access::preflight_volume_access_scope_with_report(
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
    ) -> Result<crate::access::ScopedAccessGuard> {
        crate::access::preflight_access_scope_checked_with_volume_report(
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

    fn as_tsv(&self, prefix: &str, worker: &str) -> String {
        if let Some(volume) = self.volume_report.volume_for_path(&self.path) {
            format!(
                "{}\tworker={}\tpath={}\tintent={}\tvolume-id={}\tstable-id={}\tclass={}\tmount={}\treachable={}\tread-only={}\treason=cached-volume-report",
                escape_tsv_field(prefix),
                escape_tsv_field(worker),
                escape_tsv_field(&self.path.to_string_lossy()),
                self.intent.as_str(),
                volume.id.0,
                escape_tsv_field(&volume.stable_identity),
                volume.kind.as_str(),
                volume.mount_state.as_str(),
                format_optional_bool(volume.reachable),
                volume.read_only,
            )
        } else {
            format!(
                "{}\tworker={}\tpath={}\tintent={}\tvolume-id=-\tstable-id=-\tclass=-\tmount=-\treachable=-\tread-only=-\treason=no-containing-volume",
                escape_tsv_field(prefix),
                escape_tsv_field(worker),
                escape_tsv_field(&self.path.to_string_lossy()),
                self.intent.as_str(),
            )
        }
    }
}

fn format_optional_bool(value: Option<bool>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn escape_tsv_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

#[derive(Clone)]
struct InterfaceAccessReports {
    entries: Vec<InterfaceAccessReport>,
}

impl InterfaceAccessReports {
    fn read_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Result<Self> {
        let entries = paths
            .into_iter()
            .map(|path| {
                InterfaceAccessReport::new_checked(
                    path.to_path_buf(),
                    AccessIntent::Read,
                    || Ok(()),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { entries })
    }

    fn preflight_volumes(&self, worker: &str) -> Result<()> {
        for entry in &self.entries {
            entry.preflight_volume(worker)?;
        }
        Ok(())
    }

    fn access_checked(
        &self,
        worker: &str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<crate::access::ScopedAccessGuard>> {
        let mut guards = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            check_control()?;
            guards.push(entry.access_checked(worker, &mut check_control)?);
        }
        check_control()?;
        Ok(guards)
    }

    fn first_volume(&self) -> Option<VolumeId> {
        self.entries.iter().find_map(InterfaceAccessReport::volume)
    }
}

fn ui_fileprovider_state_file_exists(path: &Path, worker: &str) -> Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(GfmError::io(
            path,
            format!("{worker} state metadata unavailable: {err}"),
        )),
    }
}

fn read_directory_with_access(path: &Path, worker: &'static str) -> Result<DirectoryPage> {
    let access_report =
        InterfaceAccessReport::new_checked(path.to_path_buf(), AccessIntent::Read, || Ok(()))?;
    access_report.preflight_volume(worker)?;
    let volume = access_report.volume();
    let path = access_report.path.clone();
    crate::runtime::run_volume_task_cancellable(
        volume,
        Priority::Visible,
        worker,
        move |cancellation| {
            cancellation.check()?;
            let _access = access_report.access_checked(worker, || cancellation.check())?;
            cancellation.check()?;
            read_directory_checked(&path, || cancellation.check())
        },
    )
}

fn read_ui_progress_snapshots(path: &Path) -> Result<Vec<JobProgressSnapshot>> {
    read_ui_progress_snapshots_with(path, JobProgressStore::read)
}

fn read_ui_restorable_progress_snapshots(path: &Path) -> Result<Vec<JobProgressSnapshot>> {
    read_ui_progress_snapshots_with(path, JobProgressStore::restorable)
}

fn read_optional_ui_payload_records(path: &Path) -> Result<HashMap<JobId, JobPayloadRecord>> {
    const WORKER: &str = "ui payload catalog";
    let access_report =
        InterfaceAccessReport::new_checked(path.to_path_buf(), AccessIntent::Read, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    let path = access_report.path.clone();
    crate::runtime::run_volume_task_cancellable_without_progress(
        volume,
        Priority::Visible,
        WORKER,
        move |cancellation| {
            cancellation.check()?;
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => return Ok(HashMap::new()),
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    return Ok(HashMap::new());
                }
                Err(err) => {
                    return Err(GfmError::io(
                        &path,
                        format!("{WORKER} metadata unavailable: {err}"),
                    ))
                }
            }
            cancellation.check()?;
            let _access = access_report.access_checked(WORKER, || cancellation.check())?;
            cancellation.check()?;
            JobPayloadCatalog::new(&path)
                .read_checked(|| cancellation.check())
                .map(|records| {
                    records
                        .into_iter()
                        .map(|record| (record.id, record))
                        .collect()
                })
        },
    )
}

fn read_ui_progress_snapshots_with(
    path: &Path,
    read: fn(&JobProgressStore) -> Result<Vec<JobProgressSnapshot>>,
) -> Result<Vec<JobProgressSnapshot>> {
    const WORKER: &str = "ui progress store";
    let access_report =
        InterfaceAccessReport::new_checked(path.to_path_buf(), AccessIntent::Read, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    let path = access_report.path.clone();
    crate::runtime::run_volume_task_cancellable_without_progress(
        volume,
        Priority::Visible,
        WORKER,
        move |cancellation| {
            cancellation.check()?;
            let _access = access_report.access_checked(WORKER, || cancellation.check())?;
            cancellation.check()?;
            let store = JobProgressStore::new(&path);
            read(&store)
        },
    )
}

fn read_ui_operation_conflicts(
    store: &crate::runtime::OperationConflictStore,
) -> Result<Vec<crate::runtime::RuntimeOperationConflict>> {
    const WORKER: &str = "ui operation conflict store";
    let access_report =
        InterfaceAccessReport::new_checked(store.path().to_path_buf(), AccessIntent::Read, || {
            Ok(())
        })?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    let path = access_report.path.clone();
    let store = crate::runtime::OperationConflictStore::new(path.clone());
    crate::runtime::run_volume_task_cancellable(
        volume,
        Priority::Visible,
        WORKER,
        move |cancellation| {
            cancellation.check()?;
            let _access = access_report.access_checked(WORKER, || cancellation.check())?;
            cancellation.check()?;
            store.read_checked(|| cancellation.check())
        },
    )
}

fn resolve_ui_operation_conflict(
    store_path: PathBuf,
    target: String,
    policy: ConflictPolicy,
) -> Result<(crate::runtime::RuntimeOperationConflict, PathBuf)> {
    const WORKER: &str = "ui operation conflict resolve";
    let store_probe = write_probe_existing_ancestor(&store_path, WORKER)?;
    let access_report =
        InterfaceAccessReport::new_checked(store_probe, AccessIntent::Write, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    crate::runtime::run_volume_task_cancellable(
        volume,
        Priority::Visible,
        WORKER,
        move |cancellation| {
            cancellation.check()?;
            let _access = access_report.access_checked(WORKER, || cancellation.check())?;
            cancellation.check()?;
            let store = crate::runtime::OperationConflictStore::new(store_path.clone());
            let resolved =
                store.resolve_checked(&target, policy.as_str(), || cancellation.check())?;
            cancellation.check()?;
            Ok((resolved, store_path))
        },
    )
}

fn run_ui_fileprovider_observed_invalidation(
    state_path: PathBuf,
    event: FileEvent,
) -> Result<FileProviderObservedInvalidation> {
    const WORKER: &str = "ui fileprovider sidebar observed invalidation";
    let state_probe = write_probe_existing_ancestor(&state_path, WORKER)?;
    let state_access_report =
        InterfaceAccessReport::new_checked(state_probe, AccessIntent::Write, || Ok(()))?;
    state_access_report.preflight_volume(WORKER)?;
    let raw_paths = fileprovider_raw_event_paths(&event);
    let raw_event_access_reports = InterfaceAccessReports::read_paths(unique_fileprovider_paths(
        raw_paths.iter().map(PathBuf::as_path),
    ))?;
    raw_event_access_reports.preflight_volumes(WORKER)?;
    let volume = state_access_report
        .volume()
        .or_else(|| raw_event_access_reports.first_volume());
    crate::runtime::run_volume_task_cancellable(
        volume,
        Priority::Visible,
        WORKER,
        move |cancellation| {
            cancellation.check()?;
            let mut access =
                vec![state_access_report.access_checked(WORKER, || cancellation.check())?];
            cancellation.check()?;
            let previous = if ui_fileprovider_state_file_exists(&state_path, WORKER)? {
                Some(FileProviderStateSnapshot::read_checked(
                    &state_path,
                    || cancellation.check(),
                )?)
            } else {
                None
            };
            let event_access_paths =
                fileprovider_event_access_paths(&event, previous.as_ref(), WORKER)?;
            let event_access_reports = InterfaceAccessReports::read_paths(
                unique_fileprovider_paths(event_access_paths.iter().map(PathBuf::as_path)),
            )?;
            access.extend(event_access_reports.access_checked(WORKER, || cancellation.check())?);
            cancellation.check()?;
            let (observed, snapshot) =
                FileProviderObservedInvalidation::evaluate(previous.as_ref(), [event])?;
            if ui_fileprovider_snapshot_changed(previous.as_ref(), &snapshot) {
                snapshot.write_checked(&state_path, || cancellation.check())?;
            }
            Ok(observed)
        },
    )
}

fn ui_fileprovider_snapshot_changed(
    previous: Option<&FileProviderStateSnapshot>,
    snapshot: &FileProviderStateSnapshot,
) -> bool {
    previous != Some(snapshot) && (previous.is_some() || !snapshot.entries.is_empty())
}

fn read_ui_fileprovider_sidebar_state(path: PathBuf) -> Result<FileProviderStateReport> {
    read_ui_fileprovider_sidebar_state_with_cancel_after_access(path, false)
}

#[cfg(test)]
fn read_ui_fileprovider_sidebar_state_cancel_after_access(
    path: PathBuf,
) -> Result<FileProviderStateReport> {
    read_ui_fileprovider_sidebar_state_with_cancel_after_access(path, true)
}

fn read_ui_fileprovider_sidebar_state_with_cancel_after_access(
    path: PathBuf,
    cancel_after_access: bool,
) -> Result<FileProviderStateReport> {
    const WORKER: &str = "ui fileprovider sidebar state";
    let access_report =
        InterfaceAccessReport::new_checked(path.clone(), AccessIntent::Read, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    crate::runtime::run_volume_task_cancellable(
        volume,
        Priority::Visible,
        WORKER,
        move |cancellation| {
            cancellation.check()?;
            let _access = access_report.access_checked(WORKER, || cancellation.check())?;
            cancellation.check()?;
            if cancel_after_access {
                cancellation.cancel();
            }
            FileProviderStateReport::read_path_checked(&path, || cancellation.check())
        },
    )
}

fn read_ui_fileprovider_sidebar_invalidation(
    path: PathBuf,
    previous: CloudStorageState,
) -> Result<FileProviderInvalidationReport> {
    const WORKER: &str = "ui fileprovider sidebar invalidation";
    let access_report =
        InterfaceAccessReport::new_checked(path.clone(), AccessIntent::Read, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    crate::runtime::run_volume_task_cancellable(
        volume,
        Priority::Visible,
        WORKER,
        move |cancellation| {
            cancellation.check()?;
            let _access = access_report.access_checked(WORKER, || cancellation.check())?;
            cancellation.check()?;
            FileProviderInvalidationReport::evaluate_checked(&path, previous, || {
                cancellation.check()
            })
        },
    )
}

fn read_ui_fileprovider_conflict(path: PathBuf) -> Result<FileProviderConflictReport> {
    const WORKER: &str = "ui fileprovider conflict";
    let access_report =
        InterfaceAccessReport::new_checked(path.clone(), AccessIntent::Read, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    crate::runtime::run_volume_task_cancellable(
        volume,
        Priority::Visible,
        WORKER,
        move |cancellation| {
            cancellation.check()?;
            let _access = access_report.access_checked(WORKER, || cancellation.check())?;
            cancellation.check()?;
            FileProviderConflictReport::read_path_checked(&path, || cancellation.check())
        },
    )
}

#[cfg(test)]
fn preflight_ui_fileprovider_read_checked(
    path: &Path,
    worker: &'static str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<crate::access::ScopedAccessGuard> {
    let access_report = InterfaceAccessReport::new_checked(
        path.to_path_buf(),
        AccessIntent::Read,
        &mut check_control,
    )?;
    access_report.access_checked(worker, check_control)
}

fn app_launch_spec(path: Option<String>) -> Result<AppLaunchSpec> {
    app_launch_spec_checked(path, || Ok(()))
}

fn app_launch_spec_checked(
    path: Option<String>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<AppLaunchSpec> {
    check_control()?;
    let mut spec = path
        .map(AppLaunchSpec::new)
        .unwrap_or_default()
        .with_sidebar_path_snapshot(SidebarPathSnapshot::discover())
        .with_sidebar_volumes(native_sidebar_volumes_checked(&mut check_control)?);
    check_control()?;
    if let Some(store) = crate::runtime::runtime_progress_store() {
        let payloads = crate::runtime::runtime_payload_catalog()
            .map(|catalog| read_optional_ui_payload_records(catalog.path()))
            .transpose()?
            .unwrap_or_default();
        let progress_surfaces = read_ui_restorable_progress_snapshots(store.path())?
            .iter()
            .filter(|snapshot| snapshot.label != "ui progress store")
            .map(|snapshot| operation_progress_contract(snapshot, payloads.get(&snapshot.id)))
            .collect();
        spec = spec.with_progress_surfaces(progress_surfaces);
    }
    if let Some(store) = crate::runtime::runtime_operation_conflict_store() {
        let conflict_inputs = read_ui_operation_conflicts(&store)?
            .iter()
            .filter(|conflict| conflict.blocks_operation)
            .map(|conflict| runtime_operation_conflict_input(conflict, Some(store.path())))
            .collect::<Vec<_>>();
        if let Some(conflict) = OperationConflictContract::from_inputs(conflict_inputs) {
            spec = spec.with_operation_conflicts(vec![conflict]);
        }
    }
    let refresh = crate::permission_refresh::refresh_permission_state(
        crate::permission_refresh::PermissionRefreshAudience::Ui,
        "window-lifecycle",
    )?;
    if let Some(refresh) = refresh {
        spec = spec.with_permission_refresh(permission_refresh_contract(&refresh));
    }
    check_control()?;
    let plan = current_permission_onboarding_checked(&mut check_control)?;
    check_control()?;
    spec = spec.with_permission_onboarding(permission_onboarding_contract(&plan));
    let admission = crate::access::worker_admission_with_volume_gate_checked(
        &spec.initial_path,
        AccessIntent::Read,
        "window initial path",
        &mut check_control,
    )?;
    let access = permission_access_contract(&admission);
    if permission_access_requires_surface(&access) {
        spec = spec.with_permission_access(access);
    }
    check_control()?;
    Ok(spec)
}

fn permission_onboarding_contract_inputs_checked(
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(
    PermissionOnboardingPlan,
    Option<gfm_mac::PermissionStateInvalidationReport>,
)> {
    check_control()?;
    let plan = current_permission_onboarding_checked(&mut check_control)?;
    check_control()?;
    let refresh = crate::permission_refresh::refresh_permission_state(
        crate::permission_refresh::PermissionRefreshAudience::Ui,
        "permission-onboarding",
    )?;
    check_control()?;
    Ok((plan, refresh))
}

fn permission_refresh_contract(
    report: &gfm_mac::PermissionStateInvalidationReport,
) -> PermissionRefreshContract {
    PermissionRefreshContract::new(
        report.initialized,
        report.changed.len(),
        report.refresh_ui,
        report.refresh_workers,
        report.refresh_operations,
    )
    .with_changes(
        report
            .changed
            .iter()
            .map(|change| PermissionRefreshChangeContract {
                scope: change.scope.as_str().to_string(),
                kind: change.kind.as_str().to_string(),
                previous: change
                    .previous
                    .map(|state| state.as_str().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                current: change.current.as_str().to_string(),
                path: change.path.display().to_string(),
                reason: change.reason.clone(),
            })
            .collect(),
    )
}

fn permission_onboarding_contract(
    plan: &gfm_mac::PermissionOnboardingPlan,
) -> PermissionOnboardingContract {
    PermissionOnboardingContract::new(
        plan.action.as_str(),
        permission_prompt_kind(plan),
        plan.policy.prompt_mode.as_str(),
        plan.finder_parity_default,
        plan.granted_for_machine_search(),
    )
    .with_scopes(
        plan.readiness
            .iter()
            .map(|item| {
                PermissionOnboardingScopeContract::new(
                    item.scope.as_str(),
                    item.state.as_str(),
                    item.path.display().to_string(),
                    item.reason.clone(),
                )
            })
            .collect(),
    )
}

fn permission_prompt_kind(plan: &gfm_mac::PermissionOnboardingPlan) -> PermissionPromptKind {
    if denied_scope_requires_full_disk_access_guidance(plan)
        || matches!(
            plan.action,
            gfm_mac::PermissionAction::ExplainFullDiskAccess
        )
    {
        return PermissionPromptKind::FullDiskAccess;
    }
    match plan.action {
        gfm_mac::PermissionAction::ContinueNormally => PermissionPromptKind::General,
        gfm_mac::PermissionAction::ContinueDegraded => PermissionPromptKind::DegradedSearch,
        gfm_mac::PermissionAction::ExplainFullDiskAccess => PermissionPromptKind::FullDiskAccess,
        gfm_mac::PermissionAction::BlockUntilGranted => {
            if plan.denied_scopes().any(|item| {
                matches!(
                    item.scope,
                    gfm_mac::PermissionScope::Desktop
                        | gfm_mac::PermissionScope::Documents
                        | gfm_mac::PermissionScope::Downloads
                )
            }) {
                PermissionPromptKind::BookmarkAcquisition
            } else {
                PermissionPromptKind::Blocked
            }
        }
    }
}

fn denied_scope_requires_full_disk_access_guidance(
    plan: &gfm_mac::PermissionOnboardingPlan,
) -> bool {
    plan.denied_scopes().any(|item| {
        matches!(
            item.scope,
            gfm_mac::PermissionScope::Mail
                | gfm_mac::PermissionScope::Photos
                | gfm_mac::PermissionScope::FullDiskAccess
        )
    })
}

fn permission_prompt_kind_for_admission(
    admission: &SecurityWorkerAdmissionReport,
) -> PermissionPromptKind {
    if matches!(admission.access.mode, SecurityAccessMode::FullDiskAccess)
        || matches!(admission.access.action, SecurityDecisionAction::Prompt)
            && admission.access.scope == gfm_mac::ProtectedScope::FullDiskAccess
    {
        return PermissionPromptKind::FullDiskAccess;
    }
    if matches!(admission.worker_action, SecurityWorkerAction::MetadataOnly) {
        return PermissionPromptKind::DegradedSearch;
    }
    if admission.access.bookmark_required
        && (admission.needs_bookmark_access
            || matches!(admission.access.action, SecurityDecisionAction::Prompt))
    {
        return PermissionPromptKind::BookmarkAcquisition;
    }
    if matches!(
        admission.worker_action,
        SecurityWorkerAction::Prompt | SecurityWorkerAction::Deny
    ) {
        return PermissionPromptKind::Blocked;
    }
    PermissionPromptKind::General
}

fn permission_prompt_action_for_admission(
    admission: &SecurityWorkerAdmissionReport,
) -> &'static str {
    if matches!(admission.access.mode, SecurityAccessMode::FullDiskAccess)
        || matches!(admission.access.action, SecurityDecisionAction::Prompt)
            && admission.access.scope == gfm_mac::ProtectedScope::FullDiskAccess
    {
        return "open-settings";
    }
    if matches!(admission.worker_action, SecurityWorkerAction::MetadataOnly) {
        return "continue-metadata-only";
    }
    if admission.access.bookmark_required
        && (admission.needs_bookmark_access
            || matches!(admission.access.action, SecurityDecisionAction::Prompt))
    {
        return "choose-location";
    }
    if admission.reason.contains("volume access blocked") {
        return "blocked-volume";
    }
    if matches!(admission.access.probe, AccessProbeState::Missing) {
        return "blocked-missing-path";
    }
    if matches!(admission.access.probe, AccessProbeState::Denied) {
        return "blocked-denied-path";
    }
    if matches!(admission.access.probe, AccessProbeState::Unavailable) {
        return "blocked-unavailable";
    }
    if matches!(
        admission.worker_action,
        SecurityWorkerAction::Prompt | SecurityWorkerAction::Deny
    ) {
        return "blocked-unavailable";
    }
    "none"
}

fn permission_prompt_orchestration_for_admission(
    admission: &SecurityWorkerAdmissionReport,
) -> (bool, &'static str) {
    if matches!(admission.access.mode, SecurityAccessMode::FullDiskAccess)
        || matches!(admission.access.action, SecurityDecisionAction::Prompt)
            && admission.access.scope == gfm_mac::ProtectedScope::FullDiskAccess
    {
        return (true, "full-disk-access");
    }
    if admission.access.bookmark_required
        && (admission.needs_bookmark_access
            || matches!(admission.access.action, SecurityDecisionAction::Prompt))
    {
        return (true, "security-scoped-bookmark");
    }
    if matches!(admission.worker_action, SecurityWorkerAction::MetadataOnly) {
        return (true, "metadata-only");
    }
    if admission.reason.contains("volume access blocked") {
        return (false, "volume");
    }
    if matches!(admission.access.probe, AccessProbeState::Missing) {
        return (false, "missing-path");
    }
    if matches!(admission.access.probe, AccessProbeState::Denied) {
        return (false, "denied-path");
    }
    if matches!(admission.access.probe, AccessProbeState::Unavailable) {
        return (false, "unavailable");
    }
    if matches!(
        admission.worker_action,
        SecurityWorkerAction::Prompt | SecurityWorkerAction::Deny
    ) {
        return (false, "unavailable");
    }
    (false, "none")
}

fn permission_access_requires_surface(access: &PermissionAccessContract) -> bool {
    access.bookmark_required
        || access.scope == "full-disk-access"
        || access.mode == "degraded-metadata-only"
        || access.promptable
        || concrete_permission_value(&access.prompt_source)
        || concrete_permission_value(&access.prompt_action)
        || matches!(access.access_action.as_str(), "deny" | "prompt")
        || matches!(access.worker_action.as_str(), "deny" | "prompt")
        || access.probe == "denied"
        || access.probe == "unavailable"
}

fn concrete_permission_value(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed != "none"
}

fn default_current_path(path: Option<String>) -> PathBuf {
    path.map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
}

fn optional_u16(value: Option<String>, name: &str, default: u16) -> Result<u16> {
    value
        .map(|value| parse_u16(&value, name))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn optional_u32(value: Option<String>, name: &str, default: u32) -> Result<u32> {
    value
        .map(|value| parse_u32(&value, name))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_u16(value: &str, name: &str) -> Result<u16> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{name} must be an unsigned 16-bit integer")))
}

fn parse_u32(value: &str, name: &str) -> Result<u32> {
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{name} must be an unsigned 32-bit integer")))
}

fn parse_u32_arg(value: Option<String>, message: &str) -> Result<u32> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{message}; got `{value}`")))
}

fn parse_u64_arg(value: Option<String>, message: &str) -> Result<u64> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{message}; got `{value}`")))
}

fn parse_usize_arg(value: Option<String>, message: &str) -> Result<usize> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    value
        .parse()
        .map_err(|_| GfmError::Format(format!("{message}; got `{value}`")))
}

fn parse_u16_arg(value: Option<String>, message: &str) -> Result<u16> {
    let value = value.ok_or_else(|| GfmError::Format(message.to_string()))?;
    parse_u16(&value, message)
}

fn parse_bool(value: &str, name: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(GfmError::Format(format!("{name} must be true or false"))),
    }
}

fn escape_interface_field(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

fn parse_virtual_surface(value: Option<&str>) -> Result<VirtualSurface> {
    match value {
        Some("icon-grid") => Ok(VirtualSurface::IconGrid),
        Some("list-rows") => Ok(VirtualSurface::ListRows),
        Some("column-rows") => Ok(VirtualSurface::ColumnRows),
        Some("gallery-filmstrip") => Ok(VirtualSurface::GalleryFilmstrip),
        Some("search-results") => Ok(VirtualSurface::SearchResults),
        Some("trash-rows") => Ok(VirtualSurface::TrashRows),
        Some(other) => Err(GfmError::Format(format!(
            "virtual surface must be icon-grid, list-rows, column-rows, gallery-filmstrip, search-results, or trash-rows; got `{other}`"
        ))),
        None => Err(GfmError::Format(
            "ui-virtualization-contract requires a virtual surface".to_string(),
        )),
    }
}

fn parse_package_traversal_mode(value: Option<&str>) -> Result<PackageTraversalMode> {
    match value.unwrap_or(PackageTraversalMode::Opaque.as_str()) {
        "opaque" => Ok(PackageTraversalMode::Opaque),
        "traverse" => Ok(PackageTraversalMode::Traverse),
        other => Err(GfmError::Format(format!(
            "package traversal mode must be opaque or traverse; got `{other}`"
        ))),
    }
}

fn read_trash_restore_metadata(path: &Path) -> Result<BTreeMap<String, TrashEntryMetadata>> {
    const WORKER: &str = "ui trash metadata";
    let access_report =
        InterfaceAccessReport::new_checked(path.to_path_buf(), AccessIntent::Read, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    let path = access_report.path.clone();
    crate::runtime::run_volume_task_cancellable(
        volume,
        Priority::Visible,
        WORKER,
        move |cancellation| {
            cancellation.check()?;
            let _access = access_report.access_checked(WORKER, || cancellation.check())?;
            cancellation.check()?;
            parse_trash_restore_metadata_checked(&path, || cancellation.check())
        },
    )
}

fn read_finder_metadata(path: PathBuf) -> Result<FinderMetadataReport> {
    read_finder_metadata_with_cancel_after_access(path, false)
}

#[cfg(test)]
fn read_finder_metadata_cancel_after_access(path: PathBuf) -> Result<FinderMetadataReport> {
    read_finder_metadata_with_cancel_after_access(path, true)
}

fn read_finder_metadata_with_cancel_after_access(
    path: PathBuf,
    cancel_after_access: bool,
) -> Result<FinderMetadataReport> {
    const WORKER: &str = "finder metadata";
    let access_report =
        InterfaceAccessReport::new_checked(path.clone(), AccessIntent::Read, || Ok(()))?;
    access_report.preflight_volume(WORKER)?;
    let volume = access_report.volume();
    crate::runtime::run_volume_task_cancellable(
        volume,
        Priority::Visible,
        WORKER,
        move |cancellation| {
            cancellation.check()?;
            let _access = access_report.access_checked(WORKER, || cancellation.check())?;
            cancellation.check()?;
            if cancel_after_access {
                cancellation.cancel();
            }
            FinderMetadataReport::read_path_checked(path, || cancellation.check())
        },
    )
}

fn parse_trash_restore_metadata_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<BTreeMap<String, TrashEntryMetadata>> {
    check_control()?;
    let file = fs::File::open(path).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let reader = BufReader::new(file);
    let mut metadata = BTreeMap::new();
    for (line_index, line) in reader.lines().enumerate() {
        check_control()?;
        let line = line.map_err(|err| GfmError::io(path, err))?;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 6 {
            return Err(GfmError::Format(format!(
                "{}:{} expected 6 tab-separated fields: name, original_path, deleted_at, can_restore, can_delete_permanently, permission_issue",
                path.display(),
                line_index + 1
            )));
        }
        let name = fields[0].to_string();
        let original_path = (!fields[1].is_empty()).then(|| PathBuf::from(fields[1]));
        let deleted_at = (!fields[2].is_empty()).then(|| fields[2].to_string());
        let can_restore = parse_bool(fields[3], "can_restore")?;
        let can_delete_permanently = parse_bool(fields[4], "can_delete_permanently")?;
        let permission_issue = (!fields[5].is_empty()).then(|| fields[5].to_string());
        metadata.insert(
            name,
            TrashEntryMetadata {
                original_path,
                deleted_at,
                can_restore,
                can_delete_permanently,
                permission_issue,
            },
        );
        check_control()?;
    }
    check_control()?;
    Ok(metadata)
}

fn search_results_stage(stage: gfm_index::SearchStreamStage) -> SearchResultsStage {
    match stage {
        gfm_index::SearchStreamStage::Hot => SearchResultsStage::Hot,
        gfm_index::SearchStreamStage::Deep => SearchResultsStage::Deep,
    }
}

fn operation_progress_contract(
    snapshot: &JobProgressSnapshot,
    payload: Option<&JobPayloadRecord>,
) -> OperationProgressContract {
    let mut input = OperationProgressInput::new(
        snapshot.label.clone(),
        operation_progress_state(snapshot.state),
        snapshot.completed_units,
        snapshot.total_units,
        snapshot.detail.clone(),
    )
    .with_job_id(snapshot.id.value());
    if let Some(payload) = payload {
        input = input.with_payload(
            operation_progress_payload_kind(payload.kind),
            payload.payload_path.to_string_lossy(),
            payload.summary.clone(),
        );
    }
    OperationProgressContract::from_input(input)
}

fn operation_progress_state(state: JobProgressState) -> OperationProgressState {
    match state {
        JobProgressState::Planned => OperationProgressState::Planned,
        JobProgressState::Running => OperationProgressState::Running,
        JobProgressState::Paused => OperationProgressState::Paused,
        JobProgressState::Completed => OperationProgressState::Completed,
        JobProgressState::Cancelled => OperationProgressState::Cancelled,
        JobProgressState::Failed => OperationProgressState::Failed,
    }
}

fn operation_progress_payload_kind(kind: JobPayloadKind) -> OperationProgressPayloadKind {
    match kind {
        JobPayloadKind::Operation => OperationProgressPayloadKind::Operation,
        JobPayloadKind::Indexing => OperationProgressPayloadKind::Indexing,
        JobPayloadKind::Extraction => OperationProgressPayloadKind::Extraction,
        JobPayloadKind::Thumbnail => OperationProgressPayloadKind::Thumbnail,
        JobPayloadKind::Preview => OperationProgressPayloadKind::Preview,
        JobPayloadKind::Repair => OperationProgressPayloadKind::Repair,
    }
}

fn sidebar_cloud_state(state: CloudStorageState) -> SidebarCloudState {
    match state {
        CloudStorageState::LocalOnly => SidebarCloudState::None,
        CloudStorageState::Downloaded => SidebarCloudState::AvailableOffline,
        CloudStorageState::Evicted => SidebarCloudState::CloudOnly,
        CloudStorageState::Downloading => SidebarCloudState::Downloading,
        CloudStorageState::Uploading => SidebarCloudState::Syncing,
        CloudStorageState::Waiting => SidebarCloudState::Waiting,
        CloudStorageState::Conflict => SidebarCloudState::Conflict,
        CloudStorageState::Offline => SidebarCloudState::Unavailable,
        CloudStorageState::Unknown => SidebarCloudState::Waiting,
        CloudStorageState::Removed => SidebarCloudState::Unavailable,
    }
}

fn required_path(value: Option<String>, message: &str) -> Result<PathBuf> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| GfmError::Format(message.to_string()))
}

fn required_string(value: Option<String>, message: &str) -> Result<String> {
    value.ok_or_else(|| GfmError::Format(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_mac::FileProviderStateSnapshotEntry;

    fn allowed_permission_access() -> PermissionAccessContract {
        PermissionAccessContract {
            path: "/Users/me/Documents".to_string(),
            intent: "read".to_string(),
            scope: "none".to_string(),
            probe: "granted".to_string(),
            mode: "allowed".to_string(),
            access_action: "allow".to_string(),
            worker_action: "start".to_string(),
            can_touch_filesystem: true,
            bookmark_required: false,
            bookmark_access: false,
            refresh_on_permission_change: false,
            prompt_kind: PermissionPromptKind::General,
            prompt_action: "none".to_string(),
            promptable: false,
            prompt_source: "none".to_string(),
            reason: "path is directly readable".to_string(),
        }
    }

    #[test]
    fn permission_access_surface_is_not_required_for_plain_allowed_paths() {
        let access = allowed_permission_access();

        assert!(!permission_access_requires_surface(&access));
    }

    #[test]
    fn permission_access_surface_ignores_blank_prompt_source_for_plain_allowed_paths() {
        let mut access = allowed_permission_access();
        access.prompt_source = "   ".to_string();

        assert!(!permission_access_requires_surface(&access));
    }

    #[test]
    fn permission_access_surface_is_required_for_promptable_contracts() {
        let mut access = allowed_permission_access();
        access.prompt_kind = PermissionPromptKind::BookmarkAcquisition;
        access.prompt_action = "choose-location".to_string();
        access.promptable = true;
        access.prompt_source = "security-scoped-bookmark".to_string();

        assert!(permission_access_requires_surface(&access));
    }

    #[test]
    fn permission_access_surface_is_required_for_prompt_actions() {
        let mut access = allowed_permission_access();
        access.prompt_action = "choose-location".to_string();

        assert!(permission_access_requires_surface(&access));
    }

    #[test]
    fn permission_access_surface_is_required_for_non_promptable_failure_sources() {
        let mut access = allowed_permission_access();
        access.prompt_kind = PermissionPromptKind::Blocked;
        access.prompt_action = "blocked-missing-path".to_string();
        access.prompt_source = "missing-path".to_string();

        assert!(permission_access_requires_surface(&access));
    }

    #[test]
    fn permission_access_dialog_rejects_mismatched_prompt_kind_and_action() {
        let mut access = allowed_permission_access();
        access.scope = "full-disk-access".to_string();
        access.probe = "denied".to_string();
        access.mode = "full-disk-access".to_string();
        access.access_action = "prompt".to_string();
        access.worker_action = "prompt".to_string();
        access.can_touch_filesystem = false;
        access.refresh_on_permission_change = true;
        access.prompt_kind = PermissionPromptKind::FullDiskAccess;
        access.prompt_action = "choose-location".to_string();
        access.promptable = true;
        access.prompt_source = "full-disk-access".to_string();

        let err = validated_permission_access_dialog(&access).unwrap_err();
        assert!(err
            .to_string()
            .contains("pairs a prompt action with the wrong prompt kind"));
    }

    #[test]
    fn permission_access_routes_mail_and_photos_denials_to_full_disk_access_guidance() {
        for scope in [
            gfm_mac::ProtectedScope::Mail,
            gfm_mac::ProtectedScope::Photos,
        ] {
            let admission = SecurityWorkerAdmissionReport::from_access_report(
                "index worker",
                gfm_mac::SecurityScopedAccessReport {
                    path: PathBuf::from("/Users/me/Library/Group Containers/group.com.apple.mail"),
                    intent: gfm_mac::AccessIntent::Index,
                    scope,
                    probe: gfm_mac::AccessProbeState::Denied,
                    mode: SecurityAccessMode::FullDiskAccess,
                    action: gfm_mac::SecurityDecisionAction::Prompt,
                    bookmark_required: false,
                    can_read: false,
                    can_write: false,
                    least_privilege: false,
                    reason: "protected root requires Full Disk Access guidance".to_string(),
                },
            );

            let access = permission_access_contract(&admission);

            assert_eq!(access.prompt_kind, PermissionPromptKind::FullDiskAccess);
            assert_eq!(access.prompt_action, "open-settings");
            assert!(access.promptable);
            assert_eq!(access.prompt_source, "full-disk-access");
            assert!(permission_access_requires_surface(&access));
        }
    }

    #[test]
    fn permission_onboarding_contract_inputs_honor_pre_cancelled_control() {
        let err =
            permission_onboarding_contract_inputs_checked(|| Err(GfmError::Cancelled)).unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
    }

    #[test]
    fn permission_onboarding_contract_carries_machine_search_readiness() {
        let plan = PermissionOnboardingPlan {
            policy: gfm_mac::PermissionPolicy::default(),
            readiness: vec![
                gfm_mac::PermissionReadiness {
                    scope: gfm_mac::PermissionScope::Documents,
                    path: PathBuf::from("/Users/me/Documents"),
                    state: gfm_mac::PermissionState::Denied,
                    reason: "operation denied by host".to_string(),
                },
                gfm_mac::PermissionReadiness {
                    scope: gfm_mac::PermissionScope::FullDiskAccess,
                    path: PathBuf::from("/Users/me/Library/Mail"),
                    state: gfm_mac::PermissionState::Granted,
                    reason: "read probe succeeded".to_string(),
                },
            ],
            action: gfm_mac::PermissionAction::ContinueDegraded,
            finder_parity_default: true,
        };

        let contract = permission_onboarding_contract(&plan);

        assert_eq!(contract.action, "continue-degraded");
        assert_eq!(contract.prompt_kind, PermissionPromptKind::DegradedSearch);
        assert_eq!(contract.prompt_mode, "defer-until-needed");
        assert!(contract.finder_parity_default);
        assert!(!contract.machine_search_ready);
        assert_eq!(contract.scopes.len(), 2);
        assert_eq!(contract.scopes[0].scope, "documents");
        assert_eq!(contract.scopes[0].state, "denied");
        assert!(contract
            .as_tsv()
            .contains("\npermission-scope\tdocuments\tstate=denied\tpath=/Users/me/Documents\t"));
    }

    #[test]
    fn permission_onboarding_contract_routes_mail_and_photos_denials_to_full_disk_access_guidance()
    {
        for scope in [
            gfm_mac::PermissionScope::Mail,
            gfm_mac::PermissionScope::Photos,
        ] {
            let plan = PermissionOnboardingPlan {
                policy: gfm_mac::PermissionPolicy::default(),
                readiness: vec![
                    gfm_mac::PermissionReadiness {
                        scope,
                        path: PathBuf::from("/Users/me/Library/Protected"),
                        state: gfm_mac::PermissionState::Denied,
                        reason: "macOS denied read access".to_string(),
                    },
                    gfm_mac::PermissionReadiness {
                        scope: gfm_mac::PermissionScope::FullDiskAccess,
                        path: PathBuf::from("/Users/me/Library/Mail"),
                        state: gfm_mac::PermissionState::Granted,
                        reason: "read probe succeeded".to_string(),
                    },
                ],
                action: gfm_mac::PermissionAction::ContinueDegraded,
                finder_parity_default: true,
            };

            let contract = permission_onboarding_contract(&plan);

            assert_eq!(contract.action, "continue-degraded");
            assert_eq!(contract.prompt_kind, PermissionPromptKind::FullDiskAccess);
        }
    }

    #[test]
    fn app_launch_spec_checked_can_cancel_during_permission_onboarding() {
        let mut checks = 0usize;

        let err = app_launch_spec_checked(None, || {
            checks += 1;
            if checks >= 4 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert!(checks >= 4);
    }

    #[test]
    fn native_sidebar_volumes_checked_honors_pre_cancelled_control() {
        let err = native_sidebar_volumes_checked(|| Err(GfmError::Cancelled)).unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
    }

    #[test]
    fn trash_restore_metadata_parser_honors_pre_cancelled_token_before_open() {
        let path = std::env::temp_dir().join(format!(
            "gfm-interface-trash-metadata-pre-cancelled-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let err =
            parse_trash_restore_metadata_checked(&path, || Err(GfmError::Cancelled)).unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert!(!path.exists());
    }

    #[test]
    fn trash_restore_metadata_parser_returns_cancelled_between_lines() {
        let path = std::env::temp_dir().join(format!(
            "gfm-interface-trash-metadata-cancelled-{}",
            std::process::id()
        ));
        let line = "report.md\t/Users/me/report.md\t2026-08-30\ttrue\ttrue\t\n";
        let text = line.repeat(128);
        std::fs::write(&path, &text).unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut checks = 0usize;

        let err = parse_trash_restore_metadata_checked(&path, || {
            checks += 1;
            if checks >= 6 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unique_fileprovider_paths_preserves_first_occurrence_order() {
        let first = PathBuf::from("/tmp/gfm/a");
        let second = PathBuf::from("/tmp/gfm/b");
        let third = PathBuf::from("/tmp/gfm/c");
        let paths = [
            first.as_path(),
            second.as_path(),
            first.as_path(),
            third.as_path(),
            second.as_path(),
        ];

        let unique = unique_fileprovider_paths(paths);

        assert_eq!(
            unique,
            vec![first.as_path(), second.as_path(), third.as_path()]
        );
    }

    #[test]
    fn tracked_removed_fileprovider_event_uses_existing_ancestor_without_path_probe() {
        let root = env::temp_dir().join(format!(
            "gfm-interface-fileprovider-event-access-{}",
            std::process::id()
        ));
        let tracked = root.join("Remote.icloud").join("Gone.md");
        std::fs::create_dir_all(tracked.parent().unwrap()).unwrap();
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: tracked.clone(),
                state: CloudStorageState::Evicted,
                signature: None,
            }],
        };
        std::fs::remove_dir_all(tracked.parent().unwrap()).unwrap();

        let access_path =
            fileprovider_event_access_path(&tracked, Some(&previous), "ui test").unwrap();

        assert_eq!(access_path, root);
        assert!(!tracked.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_event_access_checked_honors_pre_cancelled_control() {
        let event = FileEvent::new(
            env::temp_dir().join("gfm-interface-fileprovider-pre-cancelled"),
            FileEventKind::Modify,
        );

        let result = retain_fileprovider_event_access_checked(
            &event,
            None,
            "ui fileprovider sidebar observed invalidation",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
    }

    #[test]
    fn ui_fileprovider_read_checked_honors_pre_cancelled_control() {
        let path = env::temp_dir().join(format!(
            "gfm-interface-fileprovider-read-pre-cancelled-{}",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);

        let result =
            preflight_ui_fileprovider_read_checked(&path, "ui fileprovider sidebar state", || {
                Err(GfmError::Cancelled)
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn ui_fileprovider_sidebar_state_can_cancel_after_access_before_native_read() {
        let root = env::temp_dir().join(format!(
            "gfm-interface-fileprovider-read-cancel-after-access-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("Remote.icloud");
        fs::write(&path, "remote").unwrap();

        let result = read_ui_fileprovider_sidebar_state_cancel_after_access(path);

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn finder_metadata_can_cancel_after_access_before_metadata_read() {
        let root = env::temp_dir().join(format!(
            "gfm-interface-finder-metadata-cancel-after-access-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("Report.md");
        fs::write(&path, "report").unwrap();

        let result = read_finder_metadata_cancel_after_access(path);

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ui_fileprovider_state_file_probe_preserves_unavailable_metadata() {
        let root = env::temp_dir().join(format!(
            "gfm-interface-fileprovider-state-probe-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let state = root.join("fileprovider-state.tsv");
        fs::write(&state, b"gfm-fileprovider-state-v1\n").unwrap();
        let missing = root.join("missing-state.tsv");
        let unprobeable = root.join("fileprovider-state-unavailable".repeat(16));

        assert!(ui_fileprovider_state_file_exists(&state, "ui test").unwrap());
        assert!(!ui_fileprovider_state_file_exists(&root, "ui test").unwrap());
        assert!(!ui_fileprovider_state_file_exists(&missing, "ui test").unwrap());
        let err = ui_fileprovider_state_file_exists(&unprobeable, "ui test").unwrap_err();
        assert!(err
            .to_string()
            .contains("ui test state metadata unavailable"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interface_state_write_refuses_unreachable_volume_before_metadata_probe() {
        let root = env::temp_dir().join(format!(
            "gfm-interface-state-write-unreachable-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let state = root.join("interface-state-unavailable".repeat(16));

        let err = write_probe_existing_ancestor(&state, "interface state write")
            .expect_err("unreachable interface state write was admitted");

        assert!(
            err.to_string().contains(
                "interface state write volume access blocked: unreachable volume network"
            ),
            "{err}"
        );
        assert!(
            !err.to_string()
                .contains("interface write path metadata unavailable"),
            "{err}"
        );
        assert!(!state.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
