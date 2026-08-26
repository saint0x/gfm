use gfm_fs::{
    read_directory, scan_tree, FinderMetadataReport, PackageTraversalMode, PackageTraversalReport,
    ScanOptions,
};
use gfm_index::Indexer;
use gfm_types::{FileKind, GfmError, Result};
use gfm_ui::{
    AppLaunchSpec, ColumnSource, ColumnViewContract, ColumnViewOptions, ContextMenuContract,
    ContextMenuInput, ContextSurface, DialogContract, DialogSurface, GalleryViewContract,
    GalleryViewOptions, IconViewContract, IconViewOptions, ListViewContract, ListViewOptions,
    MenuContract, SearchResultsBatch, SearchResultsContract, SearchResultsOptions,
    SearchResultsStage, SidebarContract, TitlebarContract, ToolbarContract, TrashEntryMetadata,
    TrashViewContract, TrashViewOptions, VirtualSurface, VirtualizationContract,
    WindowLifecycleContract, WindowSessionContract, WindowSessionStore,
};
use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "app" => {
            let spec = app_launch_spec(args.next());
            gfm_ui::run_native(spec)?;
        }
        "ui-contract" => {
            let spec = app_launch_spec(args.next());
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
        "ui-titlebar-contract" => {
            let spec = app_launch_spec(args.next());
            println!("{}", TitlebarContract::from_spec(&spec)?.as_tsv());
        }
        "ui-session-contract" => {
            let spec = app_launch_spec(args.next());
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
            println!("{}", SidebarContract::discover(path).as_tsv());
        }
        "ui-icon-view-contract" => {
            let path = required_path(
                args.next(),
                "ui-icon-view-contract requires a directory path",
            )?;
            let columns = optional_u16(args.next(), "columns", 6)?;
            let viewport_rows = optional_u16(args.next(), "viewport-rows", 4)?;
            let scroll_row = optional_u16(args.next(), "scroll-row", 0)?;
            let page = read_directory(&path)?;
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
            let page = read_directory(&path)?;
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
            let page = read_directory(&path)?;
            let selected_record = selected_name
                .as_deref()
                .and_then(|name| page.entries.iter().find(|record| record.name == name));
            let mut sources = vec![ColumnSource::new(path.clone(), page.entries.clone())
                .with_scroll_row(scroll_row)
                .with_selected(selected_record.map(|record| record.id))];
            if let Some(record) =
                selected_record.filter(|record| record.kind == FileKind::Directory)
            {
                let child_page = read_directory(&record.path)?;
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
            let page = read_directory(&path)?;
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
            let snapshot = Indexer::default().build(root)?;
            let session = snapshot.query_session();
            let batches = session
                .stream_search(&query, 50)?
                .into_iter()
                .map(|batch| SearchResultsBatch::new(search_results_stage(batch.stage), batch.hits))
                .collect();
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
            let page = read_directory(&path)?;
            let metadata = metadata_path
                .as_ref()
                .map(read_trash_restore_metadata)
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
            let page = scan_tree(&root, options.clone())?;
            let report = PackageTraversalReport::from_page(&page, &options.package_policy);
            println!("{}", report.as_tsv());
        }
        "finder-metadata" => {
            let path = required_path(args.next(), "finder-metadata requires a path")?;
            println!("{}", FinderMetadataReport::read_path(path)?.as_tsv());
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

fn app_launch_spec(path: Option<String>) -> AppLaunchSpec {
    path.map(AppLaunchSpec::new).unwrap_or_default()
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

fn read_trash_restore_metadata(path: &PathBuf) -> Result<BTreeMap<String, TrashEntryMetadata>> {
    let text = std::fs::read_to_string(path).map_err(|err| GfmError::io(path, err))?;
    let mut metadata = BTreeMap::new();
    for (line_index, line) in text.lines().enumerate() {
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
    }
    Ok(metadata)
}

fn search_results_stage(stage: gfm_index::SearchStreamStage) -> SearchResultsStage {
    match stage {
        gfm_index::SearchStreamStage::Hot => SearchResultsStage::Hot,
        gfm_index::SearchStreamStage::Deep => SearchResultsStage::Deep,
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
