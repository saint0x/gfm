use crate::access::{
    preflight_access_scope_checked_with_volume_report, preflight_volume_access_scope_with_report,
    ScopedAccessGuard,
};
use crate::content::{run_content_search, run_content_search_with_volume_report};
use crate::extract::extraction_budget_profile_from_volume_report;
use crate::runtime::run_retriable_volume_task_cancellable_with_payload_path;
use crate::{parse_required_scheduling_pressure, parse_usize_arg, required_path, required_string};
use gfm_content::Extractor;
use gfm_index::{
    Indexer, LiveIndex, ProviderMetadataInvalidationReport, SearchLookupBudget,
    SearchMetadataPosting, SearchPrefixPosting, SearchQuery, SearchRecordColumns,
    SearchStreamStage, SearchVolumeScope, SidecarIndexQuerySession, SidecarQueryImport,
    SidecarQuerySessionReport,
};
use gfm_jobs::Cancellation;
use gfm_jobs::Priority;
use gfm_mac::{AccessIntent, VolumeDiscoveryReport};
use gfm_store::{
    atomic_write_checked, ContentArchive, ContentArchiveManifest, MetadataField,
    MmapContentArchive, MmapContentSet, MmapDictionary, MmapFuzzyArchive, MmapMetadataArchive,
    MmapPrefixArchive, MmapRecordArchive, MmapRecordColumns, MmapSubstringArchive,
};
use gfm_types::{
    ContentPositions, ContentPosting, FileId, FileKind, GfmError, Result, SearchHit, VolumeId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "search" | "search-retry-probe" => {
            let root = required_path(args.next(), "search requires a root path")?;
            let query = required_string(args.next(), "search requires a query string")?;
            let retry_probe = if command == "search-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let root_access = SearchRootAccessReport::new_checked(root.clone(), || Ok(()))?;
            let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
            root_access.preflight_volume("search")?;
            eprintln!(
                "{}",
                root_access.as_tsv("search-root-volume-access", "search")
            );
            if let Some(retry_access) = retry_access.as_ref() {
                retry_access.preflight_volume("search")?;
                eprintln!(
                    "{}",
                    retry_access.as_tsv("search-retry-volume-access", "search")
                );
            }
            let volume = root_access.volume().or_else(|| {
                retry_access
                    .as_ref()
                    .and_then(SearchWriteAccessReport::volume)
            });
            let hits = run_retriable_volume_task_cancellable_with_payload_path(
                volume,
                Priority::Visible,
                "search",
                root.clone(),
                move |cancellation| {
                    let root = root.clone();
                    let query = query.clone();
                    let retry_probe = retry_probe.clone();
                    let retry_access = retry_access.clone();
                    let root_access = root_access.clone();
                    cancellation.check()?;
                    if let (Some(retry_probe), Some(retry_access)) =
                        (retry_probe.as_ref(), retry_access.as_ref())
                    {
                        fail_first_search_retry_probe_attempt(
                            retry_probe,
                            retry_access,
                            "search",
                            &cancellation,
                        )?;
                    }
                    let _access = root_access.access_checked("search", || cancellation.check())?;
                    let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
                    let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
                    let session = snapshot.query_session();
                    session.search_structured_with_volume_scope_cancellable(
                        &parsed,
                        50,
                        &SearchVolumeScope::All,
                        &cancellation,
                    )
                },
            )?;
            for hit in hits {
                print_hit(&hit);
            }
        }
        "search-stream" | "search-stream-retry-probe" => {
            let root = required_path(args.next(), "search-stream requires a root path")?;
            let query = required_string(args.next(), "search-stream requires a query string")?;
            let retry_probe = if command == "search-stream-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-stream-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let root_access = SearchRootAccessReport::new_checked(root.clone(), || Ok(()))?;
            let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
            root_access.preflight_volume("search stream")?;
            eprintln!(
                "{}",
                root_access.as_tsv("search-root-volume-access", "search stream")
            );
            if let Some(retry_access) = retry_access.as_ref() {
                retry_access.preflight_volume("search stream")?;
                eprintln!(
                    "{}",
                    retry_access.as_tsv("search-retry-volume-access", "search stream")
                );
            }
            let volume = root_access.volume().or_else(|| {
                retry_access
                    .as_ref()
                    .and_then(SearchWriteAccessReport::volume)
            });
            let batches = run_retriable_volume_task_cancellable_with_payload_path(
                volume,
                Priority::Visible,
                "search stream",
                root.clone(),
                move |cancellation| {
                    let root = root.clone();
                    let query = query.clone();
                    let retry_probe = retry_probe.clone();
                    let retry_access = retry_access.clone();
                    let root_access = root_access.clone();
                    cancellation.check()?;
                    if let (Some(retry_probe), Some(retry_access)) =
                        (retry_probe.as_ref(), retry_access.as_ref())
                    {
                        fail_first_search_retry_probe_attempt(
                            retry_probe,
                            retry_access,
                            "search stream",
                            &cancellation,
                        )?;
                    }
                    let _access =
                        root_access.access_checked("search stream", || cancellation.check())?;
                    let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
                    let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
                    let session = snapshot.query_session();
                    session.stream_structured_search_cancellable(&parsed, 50, &cancellation)
                },
            )?;
            for batch in batches {
                println!("batch\t{}", stream_stage(batch.stage));
                for hit in batch.hits {
                    print_hit(&hit);
                }
            }
        }
        "search-content" => {
            let root = required_path(args.next(), "search-content requires a root path")?;
            let query = required_string(args.next(), "search-content requires a query string")?;
            let (indexed, hits) = run_content_search(root, query, Extractor::default())?;
            eprintln!("content-indexed {indexed} files");
            for hit in hits {
                print_hit(&hit);
            }
        }
        "search-content-adaptive" => {
            let root = required_path(args.next(), "search-content-adaptive requires a root path")?;
            let query = required_string(
                args.next(),
                "search-content-adaptive requires a query string",
            )?;
            let pressure = parse_required_scheduling_pressure(args, "content search")?;
            let volume_report =
                VolumeDiscoveryReport::for_containing_path_checked(&root, || Ok(()))?;
            let extractor = Extractor::with_budget_profile(
                extraction_budget_profile_from_volume_report(&root, pressure, &volume_report),
            );
            let (indexed, hits) =
                run_content_search_with_volume_report(root, query, extractor, volume_report)?;
            eprintln!("content-indexed {indexed} files");
            for hit in hits {
                print_hit(&hit);
            }
        }
        "search-content-index" | "search-content-index-retry-probe" => {
            let records =
                required_path(args.next(), "search-content-index requires a records path")?;
            let content =
                required_path(args.next(), "search-content-index requires a content path")?;
            let query =
                required_string(args.next(), "search-content-index requires a query string")?;
            let retry_probe = if command == "search-content-index-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-content-index-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let output = run_content_index_search(
                records,
                content,
                query,
                Extractor::default(),
                "content index search",
                retry_probe,
            )?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-content-index-adaptive" => {
            let records = required_path(
                args.next(),
                "search-content-index-adaptive requires a records path",
            )?;
            let content = required_path(
                args.next(),
                "search-content-index-adaptive requires a content path",
            )?;
            let query = required_string(
                args.next(),
                "search-content-index-adaptive requires a query string",
            )?;
            let pressure = parse_required_scheduling_pressure(args, "content index search")?;
            let worker = "adaptive content index search";
            let volume_reports = preflight_content_index_set_volume_access(
                &records,
                std::slice::from_ref(&content),
                worker,
            )?;
            let records_report = volume_reports.records_report()?;
            let extractor =
                Extractor::with_budget_profile(extraction_budget_profile_from_volume_report(
                    &records_report.path,
                    pressure,
                    &records_report.volume_report,
                ));
            let output = run_content_index_search_with_volume_reports(
                records,
                content,
                query,
                extractor,
                worker,
                None,
                volume_reports,
            )?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-content-index-set" | "search-content-index-set-retry-probe" => {
            let records = required_path(
                args.next(),
                "search-content-index-set requires a records path",
            )?;
            let query = required_string(
                args.next(),
                "search-content-index-set requires a query string",
            )?;
            let retry_probe = if command == "search-content-index-set-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-content-index-set-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let content_paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if content_paths.is_empty() {
                return Err(gfm_types::GfmError::Format(
                    "search-content-index-set requires at least one content archive".to_string(),
                ));
            }
            let output = run_content_index_set_search(records, content_paths, query, retry_probe)?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-content-index-set-session" | "search-content-index-set-session-retry-probe" => {
            let records = required_path(
                args.next(),
                "search-content-index-set-session requires a records path",
            )?;
            let query = required_string(
                args.next(),
                "search-content-index-set-session requires a query string",
            )?;
            let retry_probe = if command == "search-content-index-set-session-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-content-index-set-session-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let content_paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if content_paths.is_empty() {
                return Err(gfm_types::GfmError::Format(
                    "search-content-index-set-session requires at least one content archive"
                        .to_string(),
                ));
            }
            let output = run_content_index_set_session(records, content_paths, query, retry_probe)?;
            for diagnostic in output.diagnostics {
                eprintln!("{diagnostic}");
            }
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-content-index-set-session-provider-invalidation" => {
            let records = required_path(
                args.next(),
                "search-content-index-set-session-provider-invalidation requires a records path",
            )?;
            let query = required_string(
                args.next(),
                "search-content-index-set-session-provider-invalidation requires a query string",
            )?;
            let provider_path = required_path(
                args.next(),
                "search-content-index-set-session-provider-invalidation requires a provider path",
            )?;
            let previous_state = required_string(
                args.next(),
                "search-content-index-set-session-provider-invalidation requires a previous provider state",
            )?;
            let current_state = required_string(
                args.next(),
                "search-content-index-set-session-provider-invalidation requires a current provider state",
            )?;
            let reindex_metadata = parse_search_bool(
                &required_string(
                    args.next(),
                    "search-content-index-set-session-provider-invalidation requires reindex metadata",
                )?,
                "reindex metadata",
            )?;
            let state_changed = parse_search_bool(
                &required_string(
                    args.next(),
                    "search-content-index-set-session-provider-invalidation requires state changed",
                )?,
                "state changed",
            )?;
            let provider_reason = required_string(
                args.next(),
                "search-content-index-set-session-provider-invalidation requires a provider reason",
            )?;
            let content_paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if content_paths.is_empty() {
                return Err(gfm_types::GfmError::Format(
                    "search-content-index-set-session-provider-invalidation requires at least one content archive"
                        .to_string(),
                ));
            }
            let output = run_content_index_set_session_provider_invalidation(
                records,
                content_paths,
                query,
                ProviderMetadataInvalidationReport::from_provider_transition(
                    provider_path,
                    previous_state,
                    current_state,
                    reindex_metadata,
                    state_changed,
                    provider_reason,
                ),
            )?;
            for diagnostic in output.diagnostics {
                eprintln!("{diagnostic}");
            }
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-content-index-manifest" | "search-content-index-manifest-retry-probe" => {
            let records = required_path(
                args.next(),
                "search-content-index-manifest requires a records path",
            )?;
            let manifest = required_path(
                args.next(),
                "search-content-index-manifest requires a manifest path",
            )?;
            let query = required_string(
                args.next(),
                "search-content-index-manifest requires a query string",
            )?;
            let retry_probe = if command == "search-content-index-manifest-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-content-index-manifest-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let output = run_content_index_manifest_search(records, manifest, query, retry_probe)?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-content-index-manifest-session"
        | "search-content-index-manifest-session-retry-probe" => {
            let records = required_path(
                args.next(),
                "search-content-index-manifest-session requires a records path",
            )?;
            let manifest = required_path(
                args.next(),
                "search-content-index-manifest-session requires a manifest path",
            )?;
            let query = required_string(
                args.next(),
                "search-content-index-manifest-session requires a query string",
            )?;
            let retry_probe = if command == "search-content-index-manifest-session-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-content-index-manifest-session-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let output = run_content_index_manifest_session(records, manifest, query, retry_probe)?;
            for diagnostic in output.diagnostics {
                eprintln!("{diagnostic}");
            }
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-index" | "search-index-retry-probe" => {
            let index_path = required_path(args.next(), "search-index requires an index path")?;
            let query = required_string(args.next(), "search-index requires a query string")?;
            let retry_probe = if command == "search-index-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-index-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let hits = run_search_archive_read_cancellable_with_retry_probe(
                index_path,
                "search index",
                retry_probe,
                move |index_path, cancellation| {
                    let session = Indexer::default()
                        .load_query_session_cancellable(index_path, cancellation)?;
                    let parsed = SearchQuery::parse_cancellable(&query, cancellation)?;
                    session.search_structured_with_volume_scope_cancellable(
                        &parsed,
                        50,
                        &SearchVolumeScope::All,
                        cancellation,
                    )
                },
            )?;
            for hit in hits {
                print_hit(&hit);
            }
        }
        "search-index-mmap" | "search-index-mmap-retry-probe" => {
            let index_path =
                required_path(args.next(), "search-index-mmap requires an index path")?;
            let query = required_string(args.next(), "search-index-mmap requires a query string")?;
            let retry_probe = if command == "search-index-mmap-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-index-mmap-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let hits = run_search_archive_read_cancellable_with_retry_probe(
                index_path,
                "search index mmap",
                retry_probe,
                move |index_path, cancellation| {
                    let parsed = SearchQuery::parse_cancellable(&query, cancellation)?;
                    let live = LiveIndex::from_records(
                        MmapRecordArchive::open_checked(index_path, || cancellation.check())?
                            .records_checked(|| cancellation.check())?,
                    );
                    live.search_structured_with_volume_scope_cancellable(
                        &parsed,
                        50,
                        &SearchVolumeScope::All,
                        cancellation,
                    )
                },
            )?;
            for hit in hits {
                print_hit(&hit);
            }
        }
        "search-index-columns" | "search-index-columns-retry-probe" => {
            let records =
                required_path(args.next(), "search-index-columns requires a records path")?;
            let columns =
                required_path(args.next(), "search-index-columns requires a columns path")?;
            let query =
                required_string(args.next(), "search-index-columns requires a query string")?;
            let retry_probe = if command == "search-index-columns-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-index-columns-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let output = run_search_index_columns(records, columns, query, retry_probe)?;
            eprintln!("columns-indexed {}", output.columns_applied);
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-index-sidecars" | "search-index-sidecars-retry-probe" => {
            let records =
                required_path(args.next(), "search-index-sidecars requires a records path")?;
            let columns =
                required_path(args.next(), "search-index-sidecars requires a columns path")?;
            let metadata = required_path(
                args.next(),
                "search-index-sidecars requires a metadata path",
            )?;
            let prefixes = required_path(
                args.next(),
                "search-index-sidecars requires a prefixes path",
            )?;
            let substrings = required_path(
                args.next(),
                "search-index-sidecars requires a substrings path",
            )?;
            let fuzzy = required_path(args.next(), "search-index-sidecars requires a fuzzy path")?;
            let content =
                required_path(args.next(), "search-index-sidecars requires a content path")?;
            let query =
                required_string(args.next(), "search-index-sidecars requires a query string")?;
            let retry_probe = if command == "search-index-sidecars-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-index-sidecars-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let sidecars = OwnedSidecarIndexAccessPaths {
                records,
                columns,
                metadata,
                prefixes,
                substrings,
                fuzzy,
                content,
            };
            let output = run_sidecar_index_search(sidecars, query, retry_probe)?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-index-sidecars-session" | "search-index-sidecars-session-retry-probe" => {
            let records = required_path(
                args.next(),
                "search-index-sidecars-session requires a records path",
            )?;
            let columns = required_path(
                args.next(),
                "search-index-sidecars-session requires a columns path",
            )?;
            let metadata = required_path(
                args.next(),
                "search-index-sidecars-session requires a metadata path",
            )?;
            let prefixes = required_path(
                args.next(),
                "search-index-sidecars-session requires a prefixes path",
            )?;
            let substrings = required_path(
                args.next(),
                "search-index-sidecars-session requires a substrings path",
            )?;
            let fuzzy = required_path(
                args.next(),
                "search-index-sidecars-session requires a fuzzy path",
            )?;
            let content = required_path(
                args.next(),
                "search-index-sidecars-session requires a content path",
            )?;
            let query = required_string(
                args.next(),
                "search-index-sidecars-session requires a query string",
            )?;
            let retry_probe = if command == "search-index-sidecars-session-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-index-sidecars-session-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let sidecars = OwnedSidecarIndexAccessPaths {
                records,
                columns,
                metadata,
                prefixes,
                substrings,
                fuzzy,
                content,
            };
            let output = run_sidecar_index_session(sidecars, query, retry_probe)?;
            for diagnostic in output.diagnostics {
                eprintln!("{diagnostic}");
            }
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-index-sidecars-session-provider-invalidation" => {
            let records = required_path(
                args.next(),
                "search-index-sidecars-session-provider-invalidation requires a records path",
            )?;
            let columns = required_path(
                args.next(),
                "search-index-sidecars-session-provider-invalidation requires a columns path",
            )?;
            let metadata = required_path(
                args.next(),
                "search-index-sidecars-session-provider-invalidation requires a metadata path",
            )?;
            let prefixes = required_path(
                args.next(),
                "search-index-sidecars-session-provider-invalidation requires a prefixes path",
            )?;
            let substrings = required_path(
                args.next(),
                "search-index-sidecars-session-provider-invalidation requires a substrings path",
            )?;
            let fuzzy = required_path(
                args.next(),
                "search-index-sidecars-session-provider-invalidation requires a fuzzy path",
            )?;
            let content = required_path(
                args.next(),
                "search-index-sidecars-session-provider-invalidation requires a content path",
            )?;
            let query = required_string(
                args.next(),
                "search-index-sidecars-session-provider-invalidation requires a query string",
            )?;
            let provider_path = required_path(
                args.next(),
                "search-index-sidecars-session-provider-invalidation requires a provider path",
            )?;
            let previous_state = required_string(
                args.next(),
                "search-index-sidecars-session-provider-invalidation requires a previous provider state",
            )?;
            let current_state = required_string(
                args.next(),
                "search-index-sidecars-session-provider-invalidation requires a current provider state",
            )?;
            let reindex_metadata = parse_search_bool(
                &required_string(
                    args.next(),
                    "search-index-sidecars-session-provider-invalidation requires reindex metadata",
                )?,
                "reindex metadata",
            )?;
            let state_changed = parse_search_bool(
                &required_string(
                    args.next(),
                    "search-index-sidecars-session-provider-invalidation requires state changed",
                )?,
                "state changed",
            )?;
            let provider_reason = required_string(
                args.next(),
                "search-index-sidecars-session-provider-invalidation requires a provider reason",
            )?;
            let sidecars = OwnedSidecarIndexAccessPaths {
                records,
                columns,
                metadata,
                prefixes,
                substrings,
                fuzzy,
                content,
            };
            let output = run_sidecar_index_session_provider_invalidation(
                sidecars,
                query,
                ProviderMetadataInvalidationReport::from_provider_transition(
                    provider_path,
                    previous_state,
                    current_state,
                    reindex_metadata,
                    state_changed,
                    provider_reason,
                ),
            )?;
            for diagnostic in output.diagnostics {
                eprintln!("{diagnostic}");
            }
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-index-sidecars-budget" | "search-index-sidecars-budget-retry-probe" => {
            let records = required_path(
                args.next(),
                "search-index-sidecars-budget requires a records path",
            )?;
            let columns = required_path(
                args.next(),
                "search-index-sidecars-budget requires a columns path",
            )?;
            let metadata = required_path(
                args.next(),
                "search-index-sidecars-budget requires a metadata path",
            )?;
            let prefixes = required_path(
                args.next(),
                "search-index-sidecars-budget requires a prefixes path",
            )?;
            let substrings = required_path(
                args.next(),
                "search-index-sidecars-budget requires a substrings path",
            )?;
            let fuzzy = required_path(
                args.next(),
                "search-index-sidecars-budget requires a fuzzy path",
            )?;
            let content = required_path(
                args.next(),
                "search-index-sidecars-budget requires a content path",
            )?;
            let max_prefix_ids = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-prefix-ids",
            )?;
            let max_substring_grams = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-substring-grams",
            )?;
            let max_substring_ids = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-substring-ids",
            )?;
            let max_fuzzy_keys = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-fuzzy-keys",
            )?;
            let max_fuzzy_terms = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-fuzzy-terms",
            )?;
            let max_fuzzy_candidates = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-fuzzy-candidates",
            )?;
            let max_content_ids = parse_usize_arg(
                args.next(),
                "search-index-sidecars-budget requires max-content-ids",
            )?;
            let query = required_string(
                args.next(),
                "search-index-sidecars-budget requires a query string",
            )?;
            let retry_probe = if command == "search-index-sidecars-budget-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-index-sidecars-budget-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let budget = SearchLookupBudget {
                max_prefix_ids_per_term: max_prefix_ids,
                min_archive_prefix_chars: SearchLookupBudget::default().min_archive_prefix_chars,
                max_substring_grams_per_term: max_substring_grams,
                max_substring_ids_per_gram: max_substring_ids,
                max_fuzzy_keys_per_term: max_fuzzy_keys,
                max_fuzzy_terms_per_key: max_fuzzy_terms,
                max_fuzzy_candidates_per_term: max_fuzzy_candidates,
                max_metadata_ids_per_term: max_content_ids,
                max_content_ids_per_term: max_content_ids,
            };
            let sidecars = OwnedSidecarIndexAccessPaths {
                records,
                columns,
                metadata,
                prefixes,
                substrings,
                fuzzy,
                content,
            };
            let output = run_sidecar_index_budget(sidecars, query, budget, retry_probe)?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-index-sidecars-volume-scope" | "search-index-sidecars-volume-scope-retry-probe" => {
            let records = required_path(
                args.next(),
                "search-index-sidecars-volume-scope requires a records path",
            )?;
            let columns = required_path(
                args.next(),
                "search-index-sidecars-volume-scope requires a columns path",
            )?;
            let metadata = required_path(
                args.next(),
                "search-index-sidecars-volume-scope requires a metadata path",
            )?;
            let prefixes = required_path(
                args.next(),
                "search-index-sidecars-volume-scope requires a prefixes path",
            )?;
            let substrings = required_path(
                args.next(),
                "search-index-sidecars-volume-scope requires a substrings path",
            )?;
            let fuzzy = required_path(
                args.next(),
                "search-index-sidecars-volume-scope requires a fuzzy path",
            )?;
            let content = required_path(
                args.next(),
                "search-index-sidecars-volume-scope requires a content path",
            )?;
            let scope = parse_volume_scope(&required_string(
                args.next(),
                "search-index-sidecars-volume-scope requires admitted volume ids or `-`",
            )?)?;
            let query = required_string(
                args.next(),
                "search-index-sidecars-volume-scope requires a query string",
            )?;
            let retry_probe = if command == "search-index-sidecars-volume-scope-retry-probe" {
                Some(required_path(
                    args.next(),
                    "search-index-sidecars-volume-scope-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let budget = SearchLookupBudget::default();
            if volume_scope_is_empty(&scope) {
                print_empty_sidecar_session_report("sidecar-volume-scope", budget);
                return Ok(true);
            }
            let sidecars = OwnedSidecarIndexAccessPaths {
                records,
                columns,
                metadata,
                prefixes,
                substrings,
                fuzzy,
                content,
            };
            let output =
                run_sidecar_index_volume_scope(sidecars, query, scope, budget, retry_probe)?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-index-sidecars-cancel-candidates" => {
            run_sidecar_candidate_cancellation_probe()?;
        }
        "search-query-cancel-parse" => {
            run_search_query_parse_cancellation_probe()?;
        }
        "content-ids" | "content-ids-retry-probe" => {
            let content = required_path(args.next(), "content-ids requires a content path")?;
            let term = required_string(args.next(), "content-ids requires a term")?;
            let retry_probe = if command == "content-ids-retry-probe" {
                Some(required_path(
                    args.next(),
                    "content-ids-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let ids = run_content_archive_read_cancellable_with_retry_probe(
                content,
                "content ids",
                retry_probe,
                move |content, cancellation| {
                    let mut archive =
                        ContentArchive::open_checked(content, || cancellation.check())?;
                    cancellation.check()?;
                    let ids = archive.ids_for_term_checked(&term, || cancellation.check())?;
                    cancellation.check()?;
                    Ok(ids)
                },
            )?;
            print_file_ids(ids);
        }
        "content-ids-mmap" | "content-ids-mmap-retry-probe" => {
            let content = required_path(args.next(), "content-ids-mmap requires a content path")?;
            let term = required_string(args.next(), "content-ids-mmap requires a term")?;
            let retry_probe = if command == "content-ids-mmap-retry-probe" {
                Some(required_path(
                    args.next(),
                    "content-ids-mmap-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let ids = run_content_archive_read_cancellable_with_retry_probe(
                content,
                "content ids mmap",
                retry_probe,
                move |content, cancellation| {
                    let archive =
                        MmapContentArchive::open_checked(content, || cancellation.check())?;
                    cancellation.check()?;
                    let ids = archive.ids_for_term_checked(&term, || cancellation.check())?;
                    cancellation.check()?;
                    Ok(ids)
                },
            )?;
            print_file_ids(ids);
        }
        "content-ids-mmap-set" | "content-ids-mmap-set-retry-probe" => {
            let term = required_string(args.next(), "content-ids-mmap-set requires a term")?;
            let retry_probe = if command == "content-ids-mmap-set-retry-probe" {
                Some(required_path(
                    args.next(),
                    "content-ids-mmap-set-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let content_paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if content_paths.is_empty() {
                return Err(gfm_types::GfmError::Format(
                    "content-ids-mmap-set requires at least one content archive".to_string(),
                ));
            }
            let ids = run_content_archive_set_read_cancellable_with_retry_probe(
                content_paths,
                "content ids mmap set",
                retry_probe,
                move |paths, cancellation| {
                    let archive = MmapContentSet::open_checked(&paths, || cancellation.check())?;
                    cancellation.check()?;
                    let ids = archive.ids_for_term_checked(&term, || cancellation.check())?;
                    cancellation.check()?;
                    Ok(ids)
                },
            )?;
            print_file_ids(ids);
        }
        "content-ids-mmap-manifest" | "content-ids-mmap-manifest-retry-probe" => {
            let manifest = required_path(
                args.next(),
                "content-ids-mmap-manifest requires a manifest path",
            )?;
            let term = required_string(args.next(), "content-ids-mmap-manifest requires a term")?;
            let retry_probe = if command == "content-ids-mmap-manifest-retry-probe" {
                Some(required_path(
                    args.next(),
                    "content-ids-mmap-manifest-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let ids = run_content_manifest_read_cancellable_with_retry_probe(
                manifest,
                "content ids mmap manifest",
                retry_probe,
                move |manifest, cancellation| {
                    let archive =
                        MmapContentSet::open_manifest_checked(manifest, || cancellation.check())?;
                    cancellation.check()?;
                    let ids = archive.ids_for_term_checked(&term, || cancellation.check())?;
                    cancellation.check()?;
                    Ok(ids)
                },
            )?;
            print_file_ids(ids);
        }
        "content-id-block-mmap" | "content-id-block-mmap-retry-probe" => {
            let content =
                required_path(args.next(), "content-id-block-mmap requires a content path")?;
            let term = required_string(args.next(), "content-id-block-mmap requires a term")?;
            let block_index =
                parse_usize_arg(args.next(), "content-id-block-mmap requires a block index")?;
            let retry_probe = if command == "content-id-block-mmap-retry-probe" {
                Some(required_path(
                    args.next(),
                    "content-id-block-mmap-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let ids = run_content_archive_read_cancellable_with_retry_probe(
                content,
                "content id block mmap",
                retry_probe,
                move |content, cancellation| {
                    let archive =
                        MmapContentArchive::open_checked(content, || cancellation.check())?;
                    cancellation.check()?;
                    let ids = archive
                        .id_block_for_term_checked(&term, block_index, || cancellation.check())?;
                    cancellation.check()?;
                    Ok(ids)
                },
            )?;
            print_file_ids(ids);
        }
        "content-verify" => {
            let content = required_path(args.next(), "content-verify requires a content path")?;
            let report = run_content_archive_read_cancellable(
                content,
                "content verify",
                move |content, cancellation| {
                    let archive =
                        MmapContentArchive::open_checked(content, || cancellation.check())?;
                    cancellation.check()?;
                    let report = format!(
                        "content-verify\tterms={}\tbytes={}\tchecksum={}",
                        archive.indexed_terms(),
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
        "fuzzy-terms-mmap" | "fuzzy-terms-mmap-retry-probe" => {
            let fuzzy = required_path(args.next(), "fuzzy-terms-mmap requires a fuzzy path")?;
            let key = required_string(args.next(), "fuzzy-terms-mmap requires a key")?;
            let retry_probe = if command == "fuzzy-terms-mmap-retry-probe" {
                Some(required_path(
                    args.next(),
                    "fuzzy-terms-mmap-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let terms = run_search_archive_read_cancellable_with_retry_probe(
                fuzzy,
                "fuzzy terms mmap",
                retry_probe,
                move |fuzzy, cancellation| {
                    let archive = MmapFuzzyArchive::open_checked(fuzzy, || cancellation.check())?;
                    cancellation.check()?;
                    let terms = archive.terms_for_checked(&key, || cancellation.check())?;
                    cancellation.check()?;
                    Ok(terms)
                },
            )?;
            for term in terms {
                println!("{term}");
            }
        }
        "fuzzy-verify" => {
            let fuzzy = required_path(args.next(), "fuzzy-verify requires a fuzzy path")?;
            let report = run_search_archive_read_cancellable(
                fuzzy,
                "fuzzy verify",
                move |fuzzy, cancellation| {
                    let archive = MmapFuzzyArchive::open_checked(fuzzy, || cancellation.check())?;
                    cancellation.check()?;
                    let report = format!(
                        "fuzzy-verify\tkeys={}\tbytes={}\tchecksum={}",
                        archive.indexed_keys(),
                        archive.mapped_len(),
                        archive.is_checksummed()
                    );
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
            println!("{report}");
        }
        "prefix-ids-mmap" | "prefix-ids-mmap-retry-probe" => {
            let prefixes = required_path(args.next(), "prefix-ids-mmap requires a prefix path")?;
            let prefix = required_string(args.next(), "prefix-ids-mmap requires a prefix")?;
            let retry_probe = if command == "prefix-ids-mmap-retry-probe" {
                Some(required_path(
                    args.next(),
                    "prefix-ids-mmap-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let ids = run_search_archive_read_cancellable_with_retry_probe(
                prefixes,
                "prefix ids mmap",
                retry_probe,
                move |prefixes, cancellation| {
                    let archive =
                        MmapPrefixArchive::open_checked(prefixes, || cancellation.check())?;
                    cancellation.check()?;
                    let ids = archive.ids_for_checked(&prefix, || cancellation.check())?;
                    cancellation.check()?;
                    Ok(ids)
                },
            )?;
            print_file_ids(ids);
        }
        "prefix-id-block-mmap" | "prefix-id-block-mmap-retry-probe" => {
            let prefixes =
                required_path(args.next(), "prefix-id-block-mmap requires a prefix path")?;
            let prefix = required_string(args.next(), "prefix-id-block-mmap requires a prefix")?;
            let block_index =
                parse_usize_arg(args.next(), "prefix-id-block-mmap requires a block index")?;
            let retry_probe = if command == "prefix-id-block-mmap-retry-probe" {
                Some(required_path(
                    args.next(),
                    "prefix-id-block-mmap-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let ids = run_search_archive_read_cancellable_with_retry_probe(
                prefixes,
                "prefix id block mmap",
                retry_probe,
                move |prefixes, cancellation| {
                    let archive =
                        MmapPrefixArchive::open_checked(prefixes, || cancellation.check())?;
                    cancellation.check()?;
                    let ids = archive.id_block_for(&prefix, block_index)?;
                    cancellation.check()?;
                    Ok(ids)
                },
            )?;
            print_file_ids(ids);
        }
        "prefix-verify" => {
            let prefixes = required_path(args.next(), "prefix-verify requires a prefix path")?;
            let report = run_search_archive_read_cancellable(
                prefixes,
                "prefix verify",
                move |prefixes, cancellation| {
                    let archive =
                        MmapPrefixArchive::open_checked(prefixes, || cancellation.check())?;
                    cancellation.check()?;
                    let report = format!(
                        "prefix-verify\tprefixes={}\tbytes={}\tchecksum={}",
                        archive.indexed_prefixes(),
                        archive.mapped_len(),
                        archive.is_checksummed()
                    );
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
            println!("{report}");
        }
        "substring-ids-mmap" | "substring-ids-mmap-retry-probe" => {
            let substrings =
                required_path(args.next(), "substring-ids-mmap requires a substring path")?;
            let gram = required_string(args.next(), "substring-ids-mmap requires a trigram")?;
            let retry_probe = if command == "substring-ids-mmap-retry-probe" {
                Some(required_path(
                    args.next(),
                    "substring-ids-mmap-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let ids = run_search_archive_read_cancellable_with_retry_probe(
                substrings,
                "substring ids mmap",
                retry_probe,
                move |substrings, cancellation| {
                    let archive =
                        MmapSubstringArchive::open_checked(substrings, || cancellation.check())?;
                    cancellation.check()?;
                    let ids = archive.ids_for_checked(&gram, || cancellation.check())?;
                    cancellation.check()?;
                    Ok(ids)
                },
            )?;
            print_file_ids(ids);
        }
        "substring-id-block-mmap" | "substring-id-block-mmap-retry-probe" => {
            let substrings = required_path(
                args.next(),
                "substring-id-block-mmap requires a substring path",
            )?;
            let gram = required_string(args.next(), "substring-id-block-mmap requires a trigram")?;
            let block_index = parse_usize_arg(
                args.next(),
                "substring-id-block-mmap requires a block index",
            )?;
            let retry_probe = if command == "substring-id-block-mmap-retry-probe" {
                Some(required_path(
                    args.next(),
                    "substring-id-block-mmap-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let ids = run_search_archive_read_cancellable_with_retry_probe(
                substrings,
                "substring id block mmap",
                retry_probe,
                move |substrings, cancellation| {
                    let archive =
                        MmapSubstringArchive::open_checked(substrings, || cancellation.check())?;
                    cancellation.check()?;
                    let ids = archive.id_block_for(&gram, block_index)?;
                    cancellation.check()?;
                    Ok(ids)
                },
            )?;
            print_file_ids(ids);
        }
        "substring-verify" => {
            let substrings =
                required_path(args.next(), "substring-verify requires a substring path")?;
            let report = run_search_archive_read_cancellable(
                substrings,
                "substring verify",
                move |substrings, cancellation| {
                    let archive =
                        MmapSubstringArchive::open_checked(substrings, || cancellation.check())?;
                    cancellation.check()?;
                    let report = format!(
                        "substring-verify\tgrams={}\tbytes={}\tchecksum={}",
                        archive.indexed_grams(),
                        archive.mapped_len(),
                        archive.is_checksummed()
                    );
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
            println!("{report}");
        }
        "dictionary-lookup" | "dictionary-lookup-retry-probe" => {
            let dictionary =
                required_path(args.next(), "dictionary-lookup requires a dictionary path")?;
            let term = required_string(args.next(), "dictionary-lookup requires a term")?;
            let retry_probe = if command == "dictionary-lookup-retry-probe" {
                Some(required_path(
                    args.next(),
                    "dictionary-lookup-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let report = run_search_archive_read_cancellable_with_retry_probe(
                dictionary,
                "dictionary lookup",
                retry_probe,
                move |dictionary, cancellation| {
                    let archive =
                        MmapDictionary::open_checked(dictionary, || cancellation.check())?;
                    cancellation.check()?;
                    let report = match archive.find(&term)? {
                        Some(index) => format!("dictionary\tfound\tindex={index}\tterm={term}"),
                        None => format!("dictionary\tmissing\tterm={term}"),
                    };
                    cancellation.check()?;
                    Ok(report)
                },
            )?;
            println!("{report}");
        }
        "dictionary-verify" => {
            let dictionary =
                required_path(args.next(), "dictionary-verify requires a dictionary path")?;
            let report = run_search_archive_read_cancellable(
                dictionary,
                "dictionary verify",
                move |dictionary, cancellation| {
                    let archive =
                        MmapDictionary::open_checked(dictionary, || cancellation.check())?;
                    cancellation.check()?;
                    let report = format!(
                        "dictionary-verify\tterms={}\tbytes={}\tchecksum={}",
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
        "metadata-ids-mmap" | "metadata-ids-mmap-retry-probe" => {
            let metadata =
                required_path(args.next(), "metadata-ids-mmap requires a metadata path")?;
            let field = parse_metadata_field(
                &required_string(args.next(), "metadata-ids-mmap requires a field")?,
                "metadata field",
            )?;
            let term = required_string(args.next(), "metadata-ids-mmap requires a term")?;
            let retry_probe = if command == "metadata-ids-mmap-retry-probe" {
                Some(required_path(
                    args.next(),
                    "metadata-ids-mmap-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let ids = run_search_archive_read_cancellable_with_retry_probe(
                metadata,
                "metadata ids mmap",
                retry_probe,
                move |metadata, cancellation| {
                    let archive =
                        MmapMetadataArchive::open_checked(metadata, || cancellation.check())?;
                    cancellation.check()?;
                    let ids = archive.ids_for_checked(field, &term, || cancellation.check())?;
                    cancellation.check()?;
                    Ok(ids)
                },
            )?;
            print_file_ids(ids);
        }
        "metadata-id-block-mmap" | "metadata-id-block-mmap-retry-probe" => {
            let metadata = required_path(
                args.next(),
                "metadata-id-block-mmap requires a metadata path",
            )?;
            let field = parse_metadata_field(
                &required_string(args.next(), "metadata-id-block-mmap requires a field")?,
                "metadata field",
            )?;
            let term = required_string(args.next(), "metadata-id-block-mmap requires a term")?;
            let block_index =
                parse_usize_arg(args.next(), "metadata-id-block-mmap requires a block index")?;
            let retry_probe = if command == "metadata-id-block-mmap-retry-probe" {
                Some(required_path(
                    args.next(),
                    "metadata-id-block-mmap-retry-probe requires a retry probe path",
                )?)
            } else {
                None
            };
            let ids = run_search_archive_read_cancellable_with_retry_probe(
                metadata,
                "metadata id block mmap",
                retry_probe,
                move |metadata, cancellation| {
                    let archive =
                        MmapMetadataArchive::open_checked(metadata, || cancellation.check())?;
                    cancellation.check()?;
                    let ids = archive.id_block_for(field, &term, block_index)?;
                    cancellation.check()?;
                    Ok(ids)
                },
            )?;
            print_file_ids(ids);
        }
        "metadata-verify" => {
            let metadata = required_path(args.next(), "metadata-verify requires a metadata path")?;
            let report = run_search_archive_read_cancellable(
                metadata,
                "metadata verify",
                move |metadata, cancellation| {
                    let archive =
                        MmapMetadataArchive::open_checked(metadata, || cancellation.check())?;
                    cancellation.check()?;
                    let report = format!(
                        "metadata-verify\tterms={}\tbytes={}\tchecksum={}",
                        archive.indexed_terms(),
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
        _ => return Ok(false),
    }
    Ok(true)
}

fn run_sidecar_candidate_cancellation_probe() -> Result<()> {
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let import = SidecarQueryImport {
        metadata: vec![SearchMetadataPosting {
            field: gfm_index::SearchMetadataField::Tag,
            term: "needle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
        }],
        prefixes: vec![SearchPrefixPosting {
            prefix: "nee".to_string(),
            ids: vec![FileId::new(VolumeId(1), 2)],
        }],
        substrings: Vec::new(),
        fuzzy: Vec::new(),
        content: vec![ContentPosting {
            term: "needle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 3)],
            positions: vec![ContentPositions {
                id: FileId::new(VolumeId(1), 4),
                positions: vec![0],
            }],
        }],
        report: Default::default(),
    };
    match import.candidate_ids_cancellable(&cancellation) {
        Err(GfmError::Cancelled) => {
            println!(
                "search-candidate-expansion\tstatus=cancelled\treason=cancelled-before-candidate-expansion"
            );
            Ok(())
        }
        Err(error) => Err(error),
        Ok(_) => Err(GfmError::Format(
            "search candidate cancellation probe was not cancelled".to_string(),
        )),
    }
}

fn run_search_query_parse_cancellation_probe() -> Result<()> {
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let query = (0..10_000)
        .map(|index| format!("needle{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    match SearchQuery::parse_cancellable(&query, &cancellation) {
        Err(GfmError::Cancelled) => {
            println!("search-query-parse\tstatus=cancelled\treason=cancelled-before-parse");
            Ok(())
        }
        Err(error) => Err(error),
        Ok(_) => Err(GfmError::Format(
            "search query parse cancellation probe was not cancelled".to_string(),
        )),
    }
}

#[derive(Clone)]
struct ArchiveVolumeAccessReport {
    path: PathBuf,
    volume_report: VolumeDiscoveryReport,
}

#[derive(Clone)]
struct ArchiveVolumeAccessReports {
    entries: Vec<ArchiveVolumeAccessReport>,
}

impl ArchiveVolumeAccessReports {
    fn for_paths_checked<'a>(
        paths: impl IntoIterator<Item = &'a Path>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let owned_paths = paths.into_iter().map(Path::to_path_buf).collect::<Vec<_>>();
        let mut entries = Vec::new();
        for path in unique_search_paths(&owned_paths) {
            check_control()?;
            entries.push(ArchiveVolumeAccessReport {
                path: path.to_path_buf(),
                volume_report: VolumeDiscoveryReport::for_containing_path_checked(
                    path,
                    &mut check_control,
                )?,
            })
        }
        check_control()?;
        Ok(Self { entries })
    }

    fn preflight_volumes(&self, worker: &str) -> Result<()> {
        for entry in &self.entries {
            preflight_volume_access_scope_with_report(
                &entry.path,
                AccessIntent::Read,
                worker,
                &entry.volume_report,
            )?;
        }
        Ok(())
    }

    fn preflight_access_checked(
        &self,
        worker: &str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ScopedAccessGuard>> {
        self.entries
            .iter()
            .map(|entry| {
                preflight_access_scope_checked_with_volume_report(
                    &entry.path,
                    AccessIntent::Read,
                    worker,
                    &entry.volume_report,
                    &mut check_control,
                )
            })
            .collect()
    }

    fn first_volume(&self) -> Option<VolumeId> {
        self.entries.iter().find_map(|entry| {
            entry
                .volume_report
                .volume_for_path(&entry.path)
                .map(|volume| volume.id)
        })
    }

    fn emit_admission_diagnostics(&self, prefix: &str, worker: &str) {
        for entry in &self.entries {
            eprintln!("{}", entry.as_tsv(prefix, worker));
        }
    }
}

impl ArchiveVolumeAccessReport {
    fn as_tsv(&self, prefix: &str, worker: &str) -> String {
        volume_access_tsv(prefix, worker, None, &self.path, &self.volume_report)
    }
}

#[derive(Clone)]
struct SearchRootAccessReport {
    path: PathBuf,
    volume_report: VolumeDiscoveryReport,
}

impl SearchRootAccessReport {
    fn new_checked(path: PathBuf, mut check_control: impl FnMut() -> Result<()>) -> Result<Self> {
        check_control()?;
        let volume_report =
            VolumeDiscoveryReport::for_containing_path_checked(&path, &mut check_control)?;
        check_control()?;
        Ok(Self {
            path,
            volume_report,
        })
    }

    fn preflight_volume(&self, worker: &str) -> Result<()> {
        preflight_volume_access_scope_with_report(
            &self.path,
            AccessIntent::Index,
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
            AccessIntent::Index,
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
        volume_access_tsv(prefix, worker, None, &self.path, &self.volume_report)
    }
}

#[derive(Clone)]
struct SearchWriteAccessReport {
    path: PathBuf,
    volume_report: VolumeDiscoveryReport,
}

impl SearchWriteAccessReport {
    fn for_probe_checked(
        path: &Path,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        preflight_write_target_volume_checked(path, "search retry probe", &mut check_control)?;
        check_control()?;
        let path = write_probe_path(path)?.to_path_buf();
        check_control()?;
        let volume_report =
            VolumeDiscoveryReport::for_containing_path_checked(&path, &mut check_control)?;
        check_control()?;
        Ok(Self {
            path,
            volume_report,
        })
    }

    fn preflight_volume(&self, worker: &str) -> Result<()> {
        preflight_volume_access_scope_with_report(
            &self.path,
            AccessIntent::Write,
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
            AccessIntent::Write,
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
        volume_access_tsv(prefix, worker, None, &self.path, &self.volume_report)
    }
}

fn search_retry_probe_access_report(
    retry_probe: Option<&Path>,
) -> Result<Option<SearchWriteAccessReport>> {
    search_retry_probe_access_report_checked(retry_probe, || Ok(()))
}

fn search_retry_probe_access_report_checked(
    retry_probe: Option<&Path>,
    check_control: impl FnMut() -> Result<()>,
) -> Result<Option<SearchWriteAccessReport>> {
    retry_probe
        .map(|path| SearchWriteAccessReport::for_probe_checked(path, check_control))
        .transpose()
}

fn run_content_archive_read_cancellable<T>(
    path: PathBuf,
    worker: &'static str,
    read: impl Fn(PathBuf, &Cancellation) -> Result<T> + Clone + Send + Sync + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    run_content_archive_read_cancellable_with_retry_probe(path, worker, None, read)
}

fn run_content_archive_read_cancellable_with_retry_probe<T>(
    path: PathBuf,
    worker: &'static str,
    retry_probe: Option<PathBuf>,
    read: impl Fn(PathBuf, &Cancellation) -> Result<T> + Clone + Send + Sync + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    let volume_reports =
        ArchiveVolumeAccessReports::for_paths_checked([path.as_path()], || Ok(()))?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    volume_reports.preflight_volumes(worker)?;
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(worker)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        worker,
        path.clone(),
        move |cancellation| {
            let path = path.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            let read = read.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    worker,
                    &cancellation,
                )?;
            }
            let _access =
                volume_reports.preflight_access_checked(worker, || cancellation.check())?;
            cancellation.check()?;
            read(path, &cancellation)
        },
    )
}

fn run_content_archive_set_read_cancellable_with_retry_probe<T>(
    paths: Vec<PathBuf>,
    worker: &'static str,
    retry_probe: Option<PathBuf>,
    read: impl Fn(Vec<PathBuf>, &Cancellation) -> Result<T> + Clone + Send + Sync + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    let volume_reports =
        ArchiveVolumeAccessReports::for_paths_checked(paths.iter().map(PathBuf::as_path), || {
            Ok(())
        })?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    volume_reports.preflight_volumes(worker)?;
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(worker)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    let payload_path = paths.first().cloned().unwrap_or_else(|| PathBuf::from("."));
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        worker,
        payload_path,
        move |cancellation| {
            let paths = paths.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            let read = read.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    worker,
                    &cancellation,
                )?;
            }
            let _access =
                volume_reports.preflight_access_checked(worker, || cancellation.check())?;
            cancellation.check()?;
            read(paths, &cancellation)
        },
    )
}

fn run_content_manifest_read_cancellable_with_retry_probe<T>(
    manifest: PathBuf,
    worker: &'static str,
    retry_probe: Option<PathBuf>,
    read: impl Fn(PathBuf, &Cancellation) -> Result<T> + Clone + Send + Sync + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    let volume_reports =
        ArchiveVolumeAccessReports::for_paths_checked([manifest.as_path()], || Ok(()))?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    volume_reports.preflight_volumes(worker)?;
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(worker)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        worker,
        manifest.clone(),
        move |cancellation| {
            let manifest = manifest.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            let read = read.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    worker,
                    &cancellation,
                )?;
            }
            let _access = preflight_content_manifest_access_checked_with_volume_report(
                &manifest,
                worker,
                &volume_reports,
                || cancellation.check(),
            )?;
            cancellation.check()?;
            read(manifest, &cancellation)
        },
    )
}

fn run_search_archive_read_cancellable<T>(
    path: PathBuf,
    worker: &'static str,
    read: impl Fn(PathBuf, &Cancellation) -> Result<T> + Send + Sync + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    run_search_archive_read_cancellable_with_retry_probe(path, worker, None, read)
}

fn run_search_archive_read_cancellable_with_retry_probe<T>(
    path: PathBuf,
    worker: &'static str,
    retry_probe: Option<PathBuf>,
    read: impl Fn(PathBuf, &Cancellation) -> Result<T> + Send + Sync + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    let volume_reports =
        ArchiveVolumeAccessReports::for_paths_checked([path.as_path()], || Ok(()))?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    volume_reports.preflight_volumes(worker)?;
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(worker)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        worker,
        path.clone(),
        move |cancellation| {
            let path = path.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    worker,
                    &cancellation,
                )?;
            }
            let _access =
                volume_reports.preflight_access_checked(worker, || cancellation.check())?;
            cancellation.check()?;
            read(path, &cancellation)
        },
    )
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Ok(crate::parent_or_cwd(path)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("search retry probe metadata unavailable: {err}"),
        )),
    }
}

fn preflight_write_target_volume_checked(
    path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let volume_path = crate::parent_or_cwd(path);
    let volume_report =
        VolumeDiscoveryReport::for_containing_path_checked(volume_path, &mut check_control)?;
    check_control()?;
    preflight_volume_access_scope_with_report(
        volume_path,
        AccessIntent::Write,
        worker,
        &volume_report,
    )
}

fn fail_first_search_retry_probe_attempt(
    attempt_state: &Path,
    access_report: &SearchWriteAccessReport,
    worker: &str,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.check()?;
    let _access = access_report.access_checked(worker, || cancellation.check())?;
    cancellation.check()?;
    let attempts = read_search_retry_probe_attempt_checked(attempt_state, || cancellation.check())?;
    cancellation.check()?;
    write_search_retry_probe_attempt_checked(attempt_state, attempts + 1, || {
        cancellation.check()?;
        Ok(())
    })?;
    cancellation.check()?;
    if attempts == 0 {
        return Err(GfmError::Format(format!(
            "temporary {worker} retry probe busy"
        )));
    }
    Ok(())
}

fn read_search_retry_probe_attempt_checked(
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

fn write_search_retry_probe_attempt_checked(
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

struct SearchIndexColumnsOutput {
    columns_applied: usize,
    hits: Vec<SearchHit>,
}

struct ContentIndexSearchOutput {
    diagnostics: String,
    hits: Vec<SearchHit>,
}

struct ContentIndexSetSearchOutput {
    diagnostics: String,
    hits: Vec<SearchHit>,
}

struct ContentIndexSessionOutput {
    diagnostics: Vec<String>,
    hits: Vec<SearchHit>,
}

fn run_search_index_columns(
    records: PathBuf,
    columns: PathBuf,
    query: String,
    retry_probe: Option<PathBuf>,
) -> Result<SearchIndexColumnsOutput> {
    const WORKER: &str = "search index columns";
    let volume_reports =
        SearchIndexColumnsVolumeAccessReports::for_paths_checked(&records, &columns, || Ok(()))?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    volume_reports.preflight_volumes()?;
    volume_reports.emit_admission_diagnostics(WORKER);
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(WORKER)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        records.clone(),
        move |cancellation| {
            let records = records.clone();
            let columns = columns.clone();
            let query = query.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    WORKER,
                    &cancellation,
                )?;
            }
            let _access = preflight_search_index_columns_access_checked(&volume_reports, || {
                cancellation.check()
            })?;
            cancellation.check()?;
            let records = MmapRecordArchive::open_checked(records, || cancellation.check())?;
            cancellation.check()?;
            let columns = MmapRecordColumns::open_checked(columns, || cancellation.check())?;
            cancellation.check()?;
            let mut search_columns = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                cancellation.check()?;
                let column = columns.column(index)?;
                search_columns.push(SearchRecordColumns {
                    id: column.id,
                    name: column.name,
                    path: column.path,
                    extension: column.extension,
                    tags: column.tags,
                    comment: column.comment,
                });
            }
            let (live, columns_applied) = LiveIndex::from_records_with_columns(
                records.records_checked(|| cancellation.check())?,
                search_columns,
            );
            cancellation.check()?;
            let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
            Ok(SearchIndexColumnsOutput {
                columns_applied,
                hits: live.search_structured_with_volume_scope_cancellable(
                    &parsed,
                    50,
                    &SearchVolumeScope::All,
                    &cancellation,
                )?,
            })
        },
    )
}

#[derive(Clone)]
struct SearchIndexColumnsVolumeAccessReport {
    path: PathBuf,
    worker: &'static str,
    volume_report: VolumeDiscoveryReport,
}

#[derive(Clone)]
struct SearchIndexColumnsVolumeAccessReports {
    entries: [SearchIndexColumnsVolumeAccessReport; 2],
}

impl SearchIndexColumnsVolumeAccessReports {
    fn for_paths_checked(
        records: &Path,
        columns: &Path,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        Ok(Self {
            entries: [
                Self::entry_checked(records, "search index columns records", &mut check_control)?,
                Self::entry_checked(columns, "search index columns columns", &mut check_control)?,
            ],
        })
    }

    fn entry_checked(
        path: &Path,
        worker: &'static str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<SearchIndexColumnsVolumeAccessReport> {
        check_control()?;
        Ok(SearchIndexColumnsVolumeAccessReport {
            path: path.to_path_buf(),
            worker,
            volume_report: VolumeDiscoveryReport::for_containing_path_checked(
                path,
                &mut check_control,
            )?,
        })
    }

    fn preflight_volumes(&self) -> Result<()> {
        for entry in &self.entries {
            preflight_volume_access_scope_with_report(
                &entry.path,
                AccessIntent::Read,
                entry.worker,
                &entry.volume_report,
            )?;
        }
        Ok(())
    }

    fn emit_admission_diagnostics(&self, worker: &str) {
        for entry in &self.entries {
            eprintln!("{}", entry.as_tsv(worker));
        }
    }

    fn first_volume(&self) -> Option<VolumeId> {
        self.entries.iter().find_map(|entry| {
            entry
                .volume_report
                .volume_for_path(&entry.path)
                .map(|volume| volume.id)
        })
    }
}

impl SearchIndexColumnsVolumeAccessReport {
    fn as_tsv(&self, worker: &str) -> String {
        volume_access_tsv(
            "search-index-columns-volume-access",
            worker,
            Some(self.worker),
            &self.path,
            &self.volume_report,
        )
    }
}

fn run_content_index_search(
    records: PathBuf,
    content: PathBuf,
    query: String,
    extractor: Extractor,
    worker: &'static str,
    retry_probe: Option<PathBuf>,
) -> Result<ContentIndexSearchOutput> {
    let volume_reports = preflight_content_index_set_volume_access(
        &records,
        std::slice::from_ref(&content),
        worker,
    )?;
    run_content_index_search_with_volume_reports(
        records,
        content,
        query,
        extractor,
        worker,
        retry_probe,
        volume_reports,
    )
}

fn run_content_index_search_with_volume_reports(
    records: PathBuf,
    content: PathBuf,
    query: String,
    extractor: Extractor,
    worker: &'static str,
    retry_probe: Option<PathBuf>,
    volume_reports: ContentIndexVolumeAccessReports,
) -> Result<ContentIndexSearchOutput> {
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(worker)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        worker,
        records.clone(),
        move |cancellation| {
            let records = records.clone();
            let content = content.clone();
            let query = query.clone();
            let extractor = extractor.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    worker,
                    &cancellation,
                )?;
            }
            let _access =
                preflight_content_index_search_access_checked(&volume_reports, worker, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            let (live, report) = Indexer::default().load_live_with_content_for_query_cancellable(
                records,
                content,
                &query,
                &cancellation,
            )?;
            let diagnostics = format!(
            "content-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
            report.content_keys,
            report.records_loaded,
            report.records_missing,
            report.candidate_ids,
            report.full_hydration
        );
            cancellation.check()?;
            let hits =
                live.search_with_snippets_cancellable(&query, 50, &extractor, 96, &cancellation)?;
            Ok(ContentIndexSearchOutput { diagnostics, hits })
        },
    )
}

fn run_content_index_set_search(
    records: PathBuf,
    content_paths: Vec<PathBuf>,
    query: String,
    retry_probe: Option<PathBuf>,
) -> Result<ContentIndexSetSearchOutput> {
    const WORKER: &str = "content index set search";
    let volume_reports =
        preflight_content_index_set_volume_access(&records, &content_paths, WORKER)?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(WORKER)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        records.clone(),
        move |cancellation| {
            let records = records.clone();
            let content_paths = content_paths.clone();
            let query = query.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    WORKER,
                    &cancellation,
                )?;
            }
            let _access =
                preflight_content_index_set_search_access_checked(&volume_reports, WORKER, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            let archive_count = unique_search_paths(&content_paths).len();
            let (live, report) = Indexer::default().load_live_with_content_set_cancellable(
                records,
                &content_paths,
                &query,
                &cancellation,
            )?;
            let diagnostics = format!(
            "content-archives {} content-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
            archive_count,
            report.content_keys,
            report.records_loaded,
            report.records_missing,
            report.candidate_ids,
            report.full_hydration
        );
            cancellation.check()?;
            let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
            Ok(ContentIndexSetSearchOutput {
                diagnostics,
                hits: live.search_structured_with_volume_scope_cancellable(
                    &parsed,
                    50,
                    &SearchVolumeScope::All,
                    &cancellation,
                )?,
            })
        },
    )
}

fn run_content_index_set_session(
    records: PathBuf,
    content_paths: Vec<PathBuf>,
    query: String,
    retry_probe: Option<PathBuf>,
) -> Result<ContentIndexSessionOutput> {
    const WORKER: &str = "content index set session";
    let volume_reports =
        preflight_content_index_set_volume_access(&records, &content_paths, WORKER)?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(WORKER)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        records.clone(),
        move |cancellation| {
            let records = records.clone();
            let content_paths = content_paths.clone();
            let query = query.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    WORKER,
                    &cancellation,
                )?;
            }
            let _access =
                preflight_content_index_set_search_access_checked(&volume_reports, WORKER, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            let session = Indexer::default().load_content_set_query_session_cancellable(
                &records,
                &content_paths,
                &cancellation,
            )?;
            let archive_count = session.archive_count();
            let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
            let first = session.search_structured_with_budget_cancellable(
                &parsed,
                50,
                SearchLookupBudget::default(),
                &cancellation,
            )?;
            let mut diagnostics = vec![format_content_session_report(
                "content-session-first",
                archive_count,
                &first,
            )];
            let mut hits = first.search.hits;
            cancellation.check()?;
            let second = session.search_structured_with_budget_cancellable(
                &parsed,
                50,
                SearchLookupBudget::default(),
                &cancellation,
            )?;
            diagnostics.push(format_content_session_report(
                "content-session-second",
                archive_count,
                &second,
            ));
            hits.extend(second.search.hits);
            Ok(ContentIndexSessionOutput { diagnostics, hits })
        },
    )
}

fn run_content_index_set_session_provider_invalidation(
    records: PathBuf,
    content_paths: Vec<PathBuf>,
    query: String,
    provider: ProviderMetadataInvalidationReport,
) -> Result<ContentIndexSessionOutput> {
    const WORKER: &str = "content index set session provider invalidation";
    let volume_reports =
        preflight_content_index_set_volume_access(&records, &content_paths, WORKER)?;
    let volume = volume_reports.first_volume();
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        records.clone(),
        move |cancellation| {
            let records = records.clone();
            let content_paths = content_paths.clone();
            let query = query.clone();
            let provider = provider.clone();
            let _access =
                preflight_content_index_set_search_access_checked(&volume_reports, WORKER, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            let session = Indexer::default().load_content_set_query_session_cancellable(
                &records,
                &content_paths,
                &cancellation,
            )?;
            let archive_count = session.archive_count();
            let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
            let first = session.search_structured_with_budget_cancellable(
                &parsed,
                50,
                SearchLookupBudget::default(),
                &cancellation,
            )?;
            let mut diagnostics = vec![format_content_session_report(
                "content-session-provider-first",
                archive_count,
                &first,
            )];
            let mut hits = first.search.hits;

            cancellation.check()?;
            let second = session.search_structured_with_budget_cancellable(
                &parsed,
                50,
                SearchLookupBudget::default(),
                &cancellation,
            )?;
            diagnostics.push(format_content_session_report(
                "content-session-provider-second",
                archive_count,
                &second,
            ));
            hits.extend(second.search.hits);

            cancellation.check()?;
            diagnostics.push(provider.as_tsv());
            diagnostics.push(
                session
                    .apply_provider_metadata_invalidation(&provider)
                    .as_tsv(),
            );

            cancellation.check()?;
            let third = session.search_structured_with_budget_cancellable(
                &parsed,
                50,
                SearchLookupBudget::default(),
                &cancellation,
            )?;
            diagnostics.push(format_content_session_report(
                "content-session-provider-third",
                archive_count,
                &third,
            ));
            hits.extend(third.search.hits);
            Ok(ContentIndexSessionOutput { diagnostics, hits })
        },
    )
}

fn run_content_index_manifest_search(
    records: PathBuf,
    manifest: PathBuf,
    query: String,
    retry_probe: Option<PathBuf>,
) -> Result<ContentIndexSetSearchOutput> {
    const WORKER: &str = "content index manifest search";
    let volume_reports = ContentIndexManifestVolumeAccessReports::for_records_and_manifest_checked(
        &records,
        &manifest,
        || Ok(()),
    )?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    volume_reports.preflight_volumes(WORKER)?;
    volume_reports.emit_admission_diagnostics(WORKER);
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(WORKER)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        records.clone(),
        move |cancellation| {
            let records = records.clone();
            let manifest = manifest.clone();
            let query = query.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    WORKER,
                    &cancellation,
                )?;
            }
            let _access = preflight_content_index_manifest_search_access_checked(
                &manifest,
                &volume_reports,
                WORKER,
                || cancellation.check(),
            )?;
            cancellation.check()?;
            let (live, report) = Indexer::default().load_live_with_content_manifest_cancellable(
                records,
                manifest,
                &query,
                &cancellation,
            )?;
            let diagnostics = format!(
            "content-manifest-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
            report.content_keys,
            report.records_loaded,
            report.records_missing,
            report.candidate_ids,
            report.full_hydration
        );
            cancellation.check()?;
            let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
            Ok(ContentIndexSetSearchOutput {
                diagnostics,
                hits: live.search_structured_with_volume_scope_cancellable(
                    &parsed,
                    50,
                    &SearchVolumeScope::All,
                    &cancellation,
                )?,
            })
        },
    )
}

fn run_content_index_manifest_session(
    records: PathBuf,
    manifest: PathBuf,
    query: String,
    retry_probe: Option<PathBuf>,
) -> Result<ContentIndexSessionOutput> {
    const WORKER: &str = "content index manifest session";
    let volume_reports = ContentIndexManifestVolumeAccessReports::for_records_and_manifest_checked(
        &records,
        &manifest,
        || Ok(()),
    )?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    volume_reports.preflight_volumes(WORKER)?;
    volume_reports.emit_admission_diagnostics(WORKER);
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(WORKER)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        records.clone(),
        move |cancellation| {
            let records = records.clone();
            let manifest = manifest.clone();
            let query = query.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    WORKER,
                    &cancellation,
                )?;
            }
            let _access = preflight_content_index_manifest_search_access_checked(
                &manifest,
                &volume_reports,
                WORKER,
                || cancellation.check(),
            )?;
            cancellation.check()?;
            let session = Indexer::default().load_content_manifest_query_session_cancellable(
                &records,
                &manifest,
                &cancellation,
            )?;
            let archive_count = session.archive_count();
            let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
            let first = session.search_structured_with_budget_cancellable(
                &parsed,
                50,
                SearchLookupBudget::default(),
                &cancellation,
            )?;
            let mut diagnostics = vec![format_content_session_report(
                "content-manifest-session-first",
                archive_count,
                &first,
            )];
            let mut hits = first.search.hits;
            cancellation.check()?;
            let second = session.search_structured_with_budget_cancellable(
                &parsed,
                50,
                SearchLookupBudget::default(),
                &cancellation,
            )?;
            diagnostics.push(format_content_session_report(
                "content-manifest-session-second",
                archive_count,
                &second,
            ));
            hits.extend(second.search.hits);
            Ok(ContentIndexSessionOutput { diagnostics, hits })
        },
    )
}

fn preflight_content_index_set_volume_access(
    records: &Path,
    content_paths: &[PathBuf],
    worker: &str,
) -> Result<ContentIndexVolumeAccessReports> {
    let reports = ContentIndexVolumeAccessReports::for_records_and_content_paths_checked(
        records,
        content_paths,
        || Ok(()),
    )?;
    reports.preflight_volumes(worker)?;
    reports.emit_admission_diagnostics(worker);
    Ok(reports)
}

#[derive(Clone)]
struct ContentIndexVolumeAccessReport {
    path: PathBuf,
    role: &'static str,
    volume_report: VolumeDiscoveryReport,
}

#[derive(Clone)]
struct ContentIndexVolumeAccessReports {
    entries: Vec<ContentIndexVolumeAccessReport>,
}

impl ContentIndexVolumeAccessReports {
    fn for_records_and_content_paths_checked(
        records: &Path,
        content_paths: &[PathBuf],
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let mut entries = vec![ContentIndexVolumeAccessReport {
            path: records.to_path_buf(),
            role: "records",
            volume_report: VolumeDiscoveryReport::for_containing_path_checked(
                records,
                &mut check_control,
            )?,
        }];
        for path in unique_search_paths(content_paths) {
            check_control()?;
            entries.push(ContentIndexVolumeAccessReport {
                path: path.to_path_buf(),
                role: "content",
                volume_report: VolumeDiscoveryReport::for_containing_path_checked(
                    path,
                    &mut check_control,
                )?,
            });
        }
        check_control()?;
        Ok(Self { entries })
    }

    fn preflight_volumes(&self, worker: &str) -> Result<()> {
        for entry in &self.entries {
            preflight_volume_access_scope_with_report(
                &entry.path,
                AccessIntent::Read,
                &format!("{worker} {}", entry.role),
                &entry.volume_report,
            )?;
        }
        Ok(())
    }

    fn emit_admission_diagnostics(&self, worker: &str) {
        for entry in &self.entries {
            eprintln!("{}", entry.as_tsv(worker));
        }
    }

    fn first_volume(&self) -> Option<VolumeId> {
        self.entries.iter().find_map(|entry| {
            entry
                .volume_report
                .volume_for_path(&entry.path)
                .map(|volume| volume.id)
        })
    }

    fn records_report(&self) -> Result<&ContentIndexVolumeAccessReport> {
        self.entries.first().ok_or_else(|| {
            GfmError::Format("content index search records access report missing".to_string())
        })
    }
}

impl ContentIndexVolumeAccessReport {
    fn as_tsv(&self, worker: &str) -> String {
        volume_access_tsv(
            "content-index-volume-access",
            worker,
            Some(self.role),
            &self.path,
            &self.volume_report,
        )
    }
}

#[derive(Clone)]
struct ContentIndexManifestVolumeAccessReport {
    path: PathBuf,
    role: &'static str,
    volume_report: VolumeDiscoveryReport,
}

#[derive(Clone)]
struct ContentIndexManifestVolumeAccessReports {
    entries: [ContentIndexManifestVolumeAccessReport; 2],
}

impl ContentIndexManifestVolumeAccessReports {
    fn for_records_and_manifest_checked(
        records: &Path,
        manifest: &Path,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        Ok(Self {
            entries: [
                Self::entry_checked(records, "records", &mut check_control)?,
                Self::entry_checked(manifest, "manifest", &mut check_control)?,
            ],
        })
    }

    fn entry_checked(
        path: &Path,
        role: &'static str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<ContentIndexManifestVolumeAccessReport> {
        check_control()?;
        Ok(ContentIndexManifestVolumeAccessReport {
            path: path.to_path_buf(),
            role,
            volume_report: VolumeDiscoveryReport::for_containing_path_checked(
                path,
                &mut check_control,
            )?,
        })
    }

    fn preflight_volumes(&self, worker: &str) -> Result<()> {
        for entry in &self.entries {
            preflight_volume_access_scope_with_report(
                &entry.path,
                AccessIntent::Read,
                &format!("{worker} {}", entry.role),
                &entry.volume_report,
            )?;
        }
        Ok(())
    }

    fn preflight_access_checked(
        &self,
        worker: &str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ScopedAccessGuard>> {
        self.entries
            .iter()
            .map(|entry| {
                preflight_access_scope_checked_with_volume_report(
                    &entry.path,
                    AccessIntent::Read,
                    &format!("{worker} {}", entry.role),
                    &entry.volume_report,
                    &mut check_control,
                )
            })
            .collect()
    }

    fn emit_admission_diagnostics(&self, worker: &str) {
        for entry in &self.entries {
            eprintln!("{}", entry.as_tsv(worker));
        }
    }

    fn first_volume(&self) -> Option<VolumeId> {
        self.entries.iter().find_map(|entry| {
            entry
                .volume_report
                .volume_for_path(&entry.path)
                .map(|volume| volume.id)
        })
    }
}

impl ContentIndexManifestVolumeAccessReport {
    fn as_tsv(&self, worker: &str) -> String {
        volume_access_tsv(
            "content-index-manifest-volume-access",
            worker,
            Some(self.role),
            &self.path,
            &self.volume_report,
        )
    }
}

fn preflight_content_manifest_access_checked_with_volume_report(
    manifest_path: &Path,
    worker: &str,
    manifest_volume_reports: &ArchiveVolumeAccessReports,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let mut guards =
        manifest_volume_reports.preflight_access_checked(worker, &mut check_control)?;
    check_control()?;
    let manifest = ContentArchiveManifest::read_checked(manifest_path, &mut check_control)?;
    check_control()?;
    let archive_paths = manifest.resolved_archive_paths(manifest_path);
    let archive_volume_reports = ArchiveVolumeAccessReports::for_paths_checked(
        archive_paths.iter().map(PathBuf::as_path),
        &mut check_control,
    )?;
    guards.extend(archive_volume_reports.preflight_access_checked(worker, &mut check_control)?);
    check_control()?;
    Ok(guards)
}

fn preflight_search_index_columns_access_checked(
    reports: &SearchIndexColumnsVolumeAccessReports,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    reports
        .entries
        .iter()
        .map(|entry| {
            preflight_access_scope_checked_with_volume_report(
                &entry.path,
                AccessIntent::Read,
                entry.worker,
                &entry.volume_report,
                &mut check_control,
            )
        })
        .collect()
}

fn preflight_content_index_search_access_checked(
    volume_reports: &ContentIndexVolumeAccessReports,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    volume_reports
        .entries
        .iter()
        .map(|entry| {
            preflight_access_scope_checked_with_volume_report(
                &entry.path,
                AccessIntent::Read,
                &format!("{worker} {}", entry.role),
                &entry.volume_report,
                &mut check_control,
            )
        })
        .collect()
}

fn preflight_content_index_manifest_search_access_checked(
    manifest_path: &Path,
    volume_reports: &ContentIndexManifestVolumeAccessReports,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let mut guards = volume_reports.preflight_access_checked(worker, &mut check_control)?;
    check_control()?;
    let manifest = ContentArchiveManifest::read_checked(manifest_path, &mut check_control)?;
    check_control()?;
    let content_worker = format!("{worker} content");
    let archive_paths = manifest.resolved_archive_paths(manifest_path);
    let archive_volume_reports = ArchiveVolumeAccessReports::for_paths_checked(
        archive_paths.iter().map(PathBuf::as_path),
        &mut check_control,
    )?;
    archive_volume_reports.emit_admission_diagnostics(
        "content-index-manifest-archive-volume-access",
        &content_worker,
    );
    guards.extend(
        archive_volume_reports.preflight_access_checked(&content_worker, &mut check_control)?,
    );
    check_control()?;
    Ok(guards)
}

fn preflight_content_index_set_search_access_checked(
    volume_reports: &ContentIndexVolumeAccessReports,
    worker: &str,
    check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    preflight_content_index_search_access_checked(volume_reports, worker, check_control)
}

struct SidecarIndexAccessPaths<'a> {
    records: &'a Path,
    columns: &'a Path,
    metadata: &'a Path,
    prefixes: &'a Path,
    substrings: &'a Path,
    fuzzy: &'a Path,
    content: &'a Path,
}

impl<'a> SidecarIndexAccessPaths<'a> {
    fn paths_with_roles(&self) -> [(&'a Path, &'static str); 7] {
        [
            (self.records, "records"),
            (self.columns, "columns"),
            (self.metadata, "metadata"),
            (self.prefixes, "prefixes"),
            (self.substrings, "substrings"),
            (self.fuzzy, "fuzzy"),
            (self.content, "content"),
        ]
    }
}

#[derive(Clone)]
struct OwnedSidecarIndexAccessPaths {
    records: PathBuf,
    columns: PathBuf,
    metadata: PathBuf,
    prefixes: PathBuf,
    substrings: PathBuf,
    fuzzy: PathBuf,
    content: PathBuf,
}

impl OwnedSidecarIndexAccessPaths {
    fn borrowed(&self) -> SidecarIndexAccessPaths<'_> {
        SidecarIndexAccessPaths {
            records: &self.records,
            columns: &self.columns,
            metadata: &self.metadata,
            prefixes: &self.prefixes,
            substrings: &self.substrings,
            fuzzy: &self.fuzzy,
            content: &self.content,
        }
    }
}

#[derive(Clone)]
struct SidecarVolumeAccessReport {
    path: PathBuf,
    role: &'static str,
    volume_report: VolumeDiscoveryReport,
}

#[derive(Clone)]
struct SidecarVolumeAccessReports {
    entries: Vec<SidecarVolumeAccessReport>,
}

impl SidecarVolumeAccessReports {
    fn for_paths_checked(
        paths: SidecarIndexAccessPaths<'_>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let mut entries = Vec::new();
        for (path, role) in unique_sidecar_search_paths(paths.paths_with_roles()) {
            check_control()?;
            entries.push(SidecarVolumeAccessReport {
                path: path.to_path_buf(),
                role,
                volume_report: VolumeDiscoveryReport::for_containing_path_checked(
                    path,
                    &mut check_control,
                )?,
            });
        }
        check_control()?;
        Ok(Self { entries })
    }

    fn preflight_volumes(&self, worker: &str) -> Result<()> {
        for entry in &self.entries {
            preflight_volume_access_scope_with_report(
                &entry.path,
                AccessIntent::Read,
                &format!("{worker} {}", entry.role),
                &entry.volume_report,
            )?;
        }
        Ok(())
    }

    fn emit_admission_diagnostics(&self, worker: &str) {
        for entry in &self.entries {
            eprintln!("{}", entry.as_tsv(worker));
        }
    }

    fn first_volume(&self) -> Option<VolumeId> {
        self.entries.iter().find_map(|entry| {
            entry
                .volume_report
                .volume_for_path(&entry.path)
                .map(|volume| volume.id)
        })
    }

    fn admitted_volume_summary(&self) -> SidecarAdmittedVolumeSummary {
        let mut volumes = BTreeMap::new();
        for entry in &self.entries {
            if let Some(volume) = entry.volume_report.volume_for_path(&entry.path) {
                volumes
                    .entry(volume.id.0)
                    .or_insert_with(|| SidecarAdmittedVolume {
                        id: volume.id.0,
                        stable_id: volume.stable_identity.clone(),
                        root: volume.path.to_string_lossy().into_owned(),
                        label: volume.label.clone(),
                        class: volume.kind.as_str(),
                        writable: volume.writable,
                        read_only: volume.read_only,
                        reachable: format_optional_bool(volume.reachable),
                    });
            }
        }
        SidecarAdmittedVolumeSummary {
            volumes: volumes.into_values().collect(),
        }
    }
}

struct SidecarAdmittedVolume {
    id: u64,
    stable_id: String,
    root: String,
    label: String,
    class: &'static str,
    writable: bool,
    read_only: bool,
    reachable: String,
}

struct SidecarAdmittedVolumeSummary {
    volumes: Vec<SidecarAdmittedVolume>,
}

impl SidecarAdmittedVolumeSummary {
    fn empty() -> Self {
        Self {
            volumes: Vec::new(),
        }
    }

    fn as_tsv_fields(&self) -> String {
        format!(
            "admitted-volume-count={}\tadmitted-volume-ids={}\tadmitted-volume-classes={}\tadmitted-volume-roots={}\tadmitted-volume-labels={}\tadmitted-volume-stable-ids={}\tadmitted-volume-writable={}\tadmitted-volume-read-only={}\tadmitted-volume-reachable={}",
            self.volumes.len(),
            self.join(|volume| volume.id.to_string()),
            self.join(|volume| volume.class.to_string()),
            self.join(|volume| volume.root.clone()),
            self.join(|volume| volume.label.clone()),
            self.join(|volume| volume.stable_id.clone()),
            self.join(|volume| volume.writable.to_string()),
            self.join(|volume| volume.read_only.to_string()),
            self.join(|volume| volume.reachable.clone())
        )
    }

    fn join(&self, value: impl Fn(&SidecarAdmittedVolume) -> String) -> String {
        if self.volumes.is_empty() {
            return "-".to_string();
        }
        escape_tsv_field(&self.volumes.iter().map(value).collect::<Vec<_>>().join(","))
    }
}

impl SidecarVolumeAccessReport {
    fn as_tsv(&self, worker: &str) -> String {
        volume_access_tsv(
            "sidecar-volume-access",
            worker,
            Some(self.role),
            &self.path,
            &self.volume_report,
        )
    }
}

fn format_optional_bool(value: Option<bool>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn volume_access_tsv(
    prefix: &str,
    worker: &str,
    role: Option<&str>,
    path: &Path,
    volume_report: &VolumeDiscoveryReport,
) -> String {
    let escaped_prefix = escape_tsv_field(prefix);
    let escaped_worker = escape_tsv_field(worker);
    let role_field = role
        .map(|role| format!("\trole={}", escape_tsv_field(role)))
        .unwrap_or_default();
    if let Some(volume) = volume_report.volume_for_path(path) {
        format!(
            "{escaped_prefix}\tworker={escaped_worker}{role_field}\tpath={}\tvolume-id={}\tstable-id={}\tclass={}\tmount={}\treachable={}\twritable={}\tread-only={}\treason=cached-volume-report",
            escape_tsv_field(&path.to_string_lossy()),
            volume.id.0,
            escape_tsv_field(&volume.stable_identity),
            volume.kind.as_str(),
            volume.mount_state.as_str(),
            format_optional_bool(volume.reachable),
            volume.writable,
            volume.read_only,
        )
    } else {
        format!(
            "{escaped_prefix}\tworker={escaped_worker}{role_field}\tpath={}\tvolume-id=-\tstable-id=-\tclass=-\tmount=-\treachable=-\twritable=-\tread-only=-\treason=no-containing-volume",
            escape_tsv_field(&path.to_string_lossy()),
        )
    }
}

fn escape_tsv_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

struct SidecarSearchOutput {
    diagnostics: String,
    hits: Vec<SearchHit>,
}

struct SidecarSessionOutput {
    diagnostics: Vec<String>,
    hits: Vec<SearchHit>,
}

fn run_sidecar_index_search(
    paths: OwnedSidecarIndexAccessPaths,
    query: String,
    retry_probe: Option<PathBuf>,
) -> Result<SidecarSearchOutput> {
    const WORKER: &str = "sidecar search";
    let volume_reports = preflight_sidecar_index_volume_access(&paths, WORKER)?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(WORKER)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        paths.records.clone(),
        move |cancellation| {
            let paths = paths.clone();
            let query = query.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    WORKER,
                    &cancellation,
                )?;
            }
            let _access =
                preflight_sidecar_index_search_access_checked(&volume_reports, WORKER, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            let OwnedSidecarIndexAccessPaths {
                records,
                columns,
                metadata,
                prefixes,
                substrings,
                fuzzy,
                content,
            } = paths;
            let session = SidecarIndexQuerySession::open_cancellable(
                records,
                columns,
                metadata,
                prefixes,
                substrings,
                fuzzy,
                content,
                &cancellation,
            )?;
            cancellation.check()?;
            let budget = SearchLookupBudget::default();
            let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
            let report = session.search_structured_with_volume_scope_budget_cancellable(
                &parsed,
                50,
                &SearchVolumeScope::All,
                budget,
                &cancellation,
            )?;
            let hydration = &report.hydration;
            let volume_summary = volume_reports.admitted_volume_summary();
            let diagnostics = format!(
            "{}\tcolumns-indexed {} records-loaded {} records-missing {} candidate-ids {} full-hydration {} metadata-keys {} prefix-keys {} substring-keys {} fuzzy-keys {} prefix-archive-keys {} substring-archive-keys {} fuzzy-archive-keys {} content-keys {} content-cache-hits {} content-cache-misses {} metadata-budget {} substring-budget {} content-budget {}",
            volume_summary.as_tsv_fields(),
            hydration.columns_applied,
            hydration.records_loaded,
            hydration.records_missing,
            hydration.import.candidate_ids,
            hydration.import.requires_full_record_hydration,
            hydration.metadata_keys,
            hydration.prefix_keys,
            hydration.substring_keys,
            hydration.fuzzy_keys,
            session.indexed_prefixes(),
            session.indexed_substring_grams(),
            session.indexed_fuzzy_keys(),
            hydration.content_keys,
            report.content_cache_hits,
            report.content_cache_misses,
            budget.max_metadata_ids_per_term,
            budget.max_substring_ids_per_gram,
            budget.max_content_ids_per_term
        );
            Ok(SidecarSearchOutput {
                diagnostics,
                hits: report.search.hits,
            })
        },
    )
}

fn run_sidecar_index_session(
    paths: OwnedSidecarIndexAccessPaths,
    query: String,
    retry_probe: Option<PathBuf>,
) -> Result<SidecarSessionOutput> {
    const WORKER: &str = "sidecar session";
    let volume_reports = preflight_sidecar_index_volume_access(&paths, WORKER)?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(WORKER)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        paths.records.clone(),
        move |cancellation| {
            let paths = paths.clone();
            let query = query.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    WORKER,
                    &cancellation,
                )?;
            }
            let _access =
                preflight_sidecar_index_search_access_checked(&volume_reports, WORKER, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            let session = open_sidecar_index_query_session(paths, &cancellation)?;
            cancellation.check()?;
            let budget = SearchLookupBudget::default();
            let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
            let first = session.search_structured_with_volume_scope_budget_cancellable(
                &parsed,
                50,
                &SearchVolumeScope::All,
                budget,
                &cancellation,
            )?;
            cancellation.check()?;
            let second = session.search_structured_with_volume_scope_budget_cancellable(
                &parsed,
                50,
                &SearchVolumeScope::All,
                budget,
                &cancellation,
            )?;
            Ok(SidecarSessionOutput {
                diagnostics: vec![
                    format_sidecar_session_report(
                        "sidecar-session-first",
                        &session,
                        &first,
                        Some(&volume_reports),
                        budget,
                    ),
                    format_sidecar_session_report(
                        "sidecar-session-second",
                        &session,
                        &second,
                        Some(&volume_reports),
                        budget,
                    ),
                ],
                hits: second.search.hits,
            })
        },
    )
}

fn run_sidecar_index_session_provider_invalidation(
    paths: OwnedSidecarIndexAccessPaths,
    query: String,
    provider: ProviderMetadataInvalidationReport,
) -> Result<SidecarSessionOutput> {
    const WORKER: &str = "sidecar session provider invalidation";
    let volume_reports = preflight_sidecar_index_volume_access(&paths, WORKER)?;
    let volume = volume_reports.first_volume();
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        paths.records.clone(),
        move |cancellation| {
            let paths = paths.clone();
            let query = query.clone();
            let provider = provider.clone();
            let _access =
                preflight_sidecar_index_search_access_checked(&volume_reports, WORKER, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            let session = open_sidecar_index_query_session(paths, &cancellation)?;
            let budget = SearchLookupBudget::default();
            let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;

            let first = session.search_structured_with_volume_scope_budget_cancellable(
                &parsed,
                50,
                &SearchVolumeScope::All,
                budget,
                &cancellation,
            )?;
            let mut diagnostics = vec![format_sidecar_session_report(
                "sidecar-session-provider-first",
                &session,
                &first,
                Some(&volume_reports),
                budget,
            )];
            let mut hits = first.search.hits;

            cancellation.check()?;
            let second = session.search_structured_with_volume_scope_budget_cancellable(
                &parsed,
                50,
                &SearchVolumeScope::All,
                budget,
                &cancellation,
            )?;
            diagnostics.push(format_sidecar_session_report(
                "sidecar-session-provider-second",
                &session,
                &second,
                Some(&volume_reports),
                budget,
            ));
            hits.extend(second.search.hits);

            cancellation.check()?;
            diagnostics.push(provider.as_tsv());
            diagnostics.push(
                session
                    .apply_provider_metadata_invalidation(&provider)
                    .as_tsv(),
            );

            cancellation.check()?;
            let third = session.search_structured_with_volume_scope_budget_cancellable(
                &parsed,
                50,
                &SearchVolumeScope::All,
                budget,
                &cancellation,
            )?;
            diagnostics.push(format_sidecar_session_report(
                "sidecar-session-provider-third",
                &session,
                &third,
                Some(&volume_reports),
                budget,
            ));
            hits.extend(third.search.hits);
            Ok(SidecarSessionOutput { diagnostics, hits })
        },
    )
}

fn run_sidecar_index_budget(
    paths: OwnedSidecarIndexAccessPaths,
    query: String,
    budget: SearchLookupBudget,
    retry_probe: Option<PathBuf>,
) -> Result<SidecarSearchOutput> {
    const WORKER: &str = "sidecar budget";
    let volume_reports = preflight_sidecar_index_volume_access(&paths, WORKER)?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(WORKER)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        paths.records.clone(),
        move |cancellation| {
            let paths = paths.clone();
            let query = query.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    WORKER,
                    &cancellation,
                )?;
            }
            let _access =
                preflight_sidecar_index_search_access_checked(&volume_reports, WORKER, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            let session = open_sidecar_index_query_session(paths, &cancellation)?;
            cancellation.check()?;
            let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
            let report = session.search_structured_with_volume_scope_budget_cancellable(
                &parsed,
                50,
                &SearchVolumeScope::All,
                budget,
                &cancellation,
            )?;
            Ok(SidecarSearchOutput {
                diagnostics: format_sidecar_budget_report(
                    &session,
                    &report,
                    Some(&volume_reports),
                    budget,
                ),
                hits: report.search.hits,
            })
        },
    )
}

fn run_sidecar_index_volume_scope(
    paths: OwnedSidecarIndexAccessPaths,
    query: String,
    scope: SearchVolumeScope,
    budget: SearchLookupBudget,
    retry_probe: Option<PathBuf>,
) -> Result<SidecarSearchOutput> {
    const WORKER: &str = "sidecar volume scope";
    let volume_reports = preflight_sidecar_index_volume_access(&paths, WORKER)?;
    let retry_access = search_retry_probe_access_report(retry_probe.as_deref())?;
    if let Some(retry_access) = retry_access.as_ref() {
        retry_access.preflight_volume(WORKER)?;
    }
    let volume = volume_reports.first_volume().or_else(|| {
        retry_access
            .as_ref()
            .and_then(SearchWriteAccessReport::volume)
    });
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        paths.records.clone(),
        move |cancellation| {
            let paths = paths.clone();
            let query = query.clone();
            let scope = scope.clone();
            let retry_probe = retry_probe.clone();
            let retry_access = retry_access.clone();
            cancellation.check()?;
            if let (Some(retry_probe), Some(retry_access)) =
                (retry_probe.as_ref(), retry_access.as_ref())
            {
                fail_first_search_retry_probe_attempt(
                    retry_probe,
                    retry_access,
                    WORKER,
                    &cancellation,
                )?;
            }
            let _access =
                preflight_sidecar_index_search_access_checked(&volume_reports, WORKER, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            let session = open_sidecar_index_query_session(paths, &cancellation)?;
            cancellation.check()?;
            let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
            let report = session.search_structured_with_volume_scope_budget_cancellable(
                &parsed,
                50,
                &scope,
                budget,
                &cancellation,
            )?;
            Ok(SidecarSearchOutput {
                diagnostics: format_sidecar_session_report(
                    "sidecar-volume-scope",
                    &session,
                    &report,
                    Some(&volume_reports),
                    budget,
                ),
                hits: report.search.hits,
            })
        },
    )
}

fn open_sidecar_index_query_session(
    paths: OwnedSidecarIndexAccessPaths,
    cancellation: &Cancellation,
) -> Result<SidecarIndexQuerySession> {
    let OwnedSidecarIndexAccessPaths {
        records,
        columns,
        metadata,
        prefixes,
        substrings,
        fuzzy,
        content,
    } = paths;
    SidecarIndexQuerySession::open_cancellable(
        records,
        columns,
        metadata,
        prefixes,
        substrings,
        fuzzy,
        content,
        cancellation,
    )
}

fn preflight_sidecar_index_volume_access(
    paths: &OwnedSidecarIndexAccessPaths,
    worker: &str,
) -> Result<SidecarVolumeAccessReports> {
    let reports = SidecarVolumeAccessReports::for_paths_checked(paths.borrowed(), || Ok(()))?;
    reports.preflight_volumes(worker)?;
    reports.emit_admission_diagnostics(worker);
    Ok(reports)
}

fn preflight_sidecar_index_search_access_checked(
    volume_reports: &SidecarVolumeAccessReports,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::new();
    for entry in &volume_reports.entries {
        guards.push(preflight_access_scope_checked_with_volume_report(
            &entry.path,
            AccessIntent::Read,
            &format!("{worker} {}", entry.role),
            &entry.volume_report,
            &mut check_control,
        )?);
    }
    Ok(guards)
}

fn unique_search_paths(paths: &[PathBuf]) -> Vec<&Path> {
    let mut seen = BTreeSet::new();
    paths
        .iter()
        .map(PathBuf::as_path)
        .filter(|path| seen.insert((*path).to_path_buf()))
        .collect()
}

fn unique_sidecar_search_paths<'a>(
    paths: impl IntoIterator<Item = (&'a Path, &'static str)>,
) -> Vec<(&'a Path, &'static str)> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|(path, _)| seen.insert((*path).to_path_buf()))
        .collect()
}

fn parse_metadata_field(value: &str, name: &str) -> Result<MetadataField> {
    MetadataField::parse(value)
        .ok_or_else(|| gfm_types::GfmError::Format(format!("invalid {name}: {value}")))
}

fn parse_volume_scope(value: &str) -> Result<SearchVolumeScope> {
    if value == "-" || value.trim().is_empty() {
        return Ok(SearchVolumeScope::only([]));
    }
    let volumes = value
        .split(',')
        .map(|part| {
            let part = part.trim();
            let raw = part.parse::<u64>().map_err(|_| {
                GfmError::Format(format!("volume scope id `{part}` must be unsigned"))
            })?;
            Ok(VolumeId(raw))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(SearchVolumeScope::only(volumes))
}

fn print_file_ids(ids: Vec<gfm_types::FileId>) {
    for id in ids {
        println!("{}\t{}", id.volume.0, id.node);
    }
}

fn marker(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "dir",
        FileKind::File => "file",
        FileKind::Symlink => "link",
        FileKind::Other => "other",
    }
}

fn print_hit(hit: &SearchHit) {
    print!(
        "{}\t{}\t{}\t{}",
        hit.score,
        marker(hit.record.kind),
        hit.record.len,
        hit.record.path.display()
    );
    if let Some(snippet) = &hit.snippet {
        print!("\t{}", escape_output_field(&highlight_snippet(snippet)));
    }
    println!();
}

fn format_content_session_report(
    label: &str,
    content_archives: usize,
    report: &gfm_index::ContentQuerySessionReport,
) -> String {
    format!(
        "{label}\tcontent-archives={content_archives}\tcontent-keys={}\trecords-loaded={}\trecords-missing={}\tcandidate-ids={}\tfull-hydration={}\tposting-cache-hits={}\tposting-cache-misses={}\trecord-cache-hits={}\trecord-cache-misses={}\tresult-cache-hits={}\tresult-cache-misses={}",
        report.load.content_keys,
        report.load.records_loaded,
        report.load.records_missing,
        report.load.candidate_ids,
        report.load.full_hydration,
        report.posting_cache_hits,
        report.posting_cache_misses,
        report.record_cache_hits,
        report.record_cache_misses,
        report.result_cache_hits,
        report.result_cache_misses
    )
}

fn format_sidecar_session_report(
    label: &str,
    session: &SidecarIndexQuerySession,
    report: &SidecarQuerySessionReport,
    volume_reports: Option<&SidecarVolumeAccessReports>,
    budget: SearchLookupBudget,
) -> String {
    let hydration = &report.hydration;
    let volume_summary = volume_reports
        .map(SidecarVolumeAccessReports::admitted_volume_summary)
        .unwrap_or_else(SidecarAdmittedVolumeSummary::empty);
    format!(
        "{label}\t{}\trecords-indexed={}\tcolumns-indexed={}\trecords-loaded={}\trecords-missing={}\tcandidate-ids={}\tfull-hydration={}\tmetadata-keys={}\tprefix-keys={}\tsubstring-keys={}\tfuzzy-keys={}\tcontent-keys={}\tcontent-cache-hits={}\tcontent-cache-misses={}\trecord-cache-hits={}\trecord-cache-misses={}\tresult-cache-hits={}\tresult-cache-misses={}\tmetadata-budget={}\tprefix-budget={}\tsubstring-budget={}\tfuzzy-key-budget={}\tfuzzy-term-budget={}\tfuzzy-candidate-budget={}\tcontent-budget={}\tprefix-archive-keys={}\tsubstring-archive-keys={}\tfuzzy-archive-keys={}\tprefix-lookup-requests={}\tprefix-lookup-ids={}\tprefix-cache-hits={}\tprefix-cache-misses={}\tsubstring-lookup-requests={}\tsubstring-lookup-ids={}\tsubstring-cache-hits={}\tsubstring-cache-misses={}\tfuzzy-lookup-requests={}\tfuzzy-lookup-terms={}\tfuzzy-cache-hits={}\tfuzzy-cache-misses={}",
        volume_summary.as_tsv_fields(),
        session.indexed_records(),
        hydration.columns_applied,
        hydration.records_loaded,
        hydration.records_missing,
        hydration.import.candidate_ids,
        hydration.import.requires_full_record_hydration,
        hydration.metadata_keys,
        hydration.prefix_keys,
        hydration.substring_keys,
        hydration.fuzzy_keys,
        hydration.content_keys,
        report.content_cache_hits,
        report.content_cache_misses,
        report.record_cache_hits,
        report.record_cache_misses,
        report.result_cache_hits,
        report.result_cache_misses,
        budget.max_metadata_ids_per_term,
        budget.max_prefix_ids_per_term,
        budget.max_substring_ids_per_gram,
        budget.max_fuzzy_keys_per_term,
        budget.max_fuzzy_terms_per_key,
        budget.max_fuzzy_candidates_per_term,
        budget.max_content_ids_per_term,
        session.indexed_prefixes(),
        session.indexed_substring_grams(),
        session.indexed_fuzzy_keys(),
        report.search.lookup.prefix_lookup_requests,
        report.search.lookup.prefix_lookup_ids,
        report.search.lookup.prefix_cache_hits,
        report.search.lookup.prefix_cache_misses,
        report.search.lookup.substring_lookup_requests,
        report.search.lookup.substring_lookup_ids,
        report.search.lookup.substring_cache_hits,
        report.search.lookup.substring_cache_misses,
        report.search.lookup.fuzzy_lookup_requests,
        report.search.lookup.fuzzy_lookup_terms,
        report.search.lookup.fuzzy_cache_hits,
        report.search.lookup.fuzzy_cache_misses,
    )
}

fn format_sidecar_budget_report(
    session: &SidecarIndexQuerySession,
    report: &SidecarQuerySessionReport,
    volume_reports: Option<&SidecarVolumeAccessReports>,
    budget: SearchLookupBudget,
) -> String {
    let hydration = &report.hydration;
    let volume_summary = volume_reports
        .map(SidecarVolumeAccessReports::admitted_volume_summary)
        .unwrap_or_else(SidecarAdmittedVolumeSummary::empty);
    format!(
        "sidecar-budget\t{}\tcolumns-indexed={}\trecords-loaded={}\trecords-missing={}\tcandidate-ids={}\tfull-hydration={}\tmetadata-keys={}\tprefix-keys={}\tsubstring-keys={}\tfuzzy-keys={}\tcontent-keys={}\tcontent-cache-hits={}\tcontent-cache-misses={}\tmetadata-budget={}\tcontent-budget={}\tprefix-archive-keys={}\tsubstring-archive-keys={}\tfuzzy-archive-keys={}\tprefix-terms={}\tprefix-lookup-requests={}\tprefix-lookup-ids={}\tprefix-candidate-ids={}\tprefix-cache-hits={}\tprefix-cache-misses={}\tprefix-cutoff-terms={}\tprefix-truncated-terms={}\tsubstring-terms={}\tsubstring-grams={}\tsubstring-lookup-requests={}\tsubstring-lookup-ids={}\tsubstring-candidate-ids={}\tsubstring-cache-hits={}\tsubstring-cache-misses={}\tsubstring-cutoff-terms={}\tsubstring-term-truncated-grams={}\tsubstring-truncated-grams={}\tfuzzy-terms={}\tfuzzy-keys-read={}\tfuzzy-lookup-requests={}\tfuzzy-lookup-terms={}\tfuzzy-candidate-terms={}\tfuzzy-verified-candidates={}\tfuzzy-cache-hits={}\tfuzzy-cache-misses={}\tfuzzy-key-truncated-terms={}\tfuzzy-term-truncated-keys={}\tfuzzy-candidate-truncated-terms={}",
        volume_summary.as_tsv_fields(),
        hydration.columns_applied,
        hydration.records_loaded,
        hydration.records_missing,
        hydration.import.candidate_ids,
        hydration.import.requires_full_record_hydration,
        hydration.metadata_keys,
        hydration.prefix_keys,
        hydration.substring_keys,
        hydration.fuzzy_keys,
        hydration.content_keys,
        report.content_cache_hits,
        report.content_cache_misses,
        budget.max_metadata_ids_per_term,
        budget.max_content_ids_per_term,
        session.indexed_prefixes(),
        session.indexed_substring_grams(),
        session.indexed_fuzzy_keys(),
        report.search.lookup.prefix_terms,
        report.search.lookup.prefix_lookup_requests,
        report.search.lookup.prefix_lookup_ids,
        report.search.lookup.prefix_candidate_ids,
        report.search.lookup.prefix_cache_hits,
        report.search.lookup.prefix_cache_misses,
        report.search.lookup.prefix_cutoff_terms,
        report.search.lookup.prefix_truncated_terms,
        report.search.lookup.substring_terms,
        report.search.lookup.substring_grams,
        report.search.lookup.substring_lookup_requests,
        report.search.lookup.substring_lookup_ids,
        report.search.lookup.substring_candidate_ids,
        report.search.lookup.substring_cache_hits,
        report.search.lookup.substring_cache_misses,
        report.search.lookup.substring_cutoff_terms,
        report.search.lookup.substring_term_truncated_grams,
        report.search.lookup.substring_truncated_grams,
        report.search.lookup.fuzzy_terms,
        report.search.lookup.fuzzy_keys,
        report.search.lookup.fuzzy_lookup_requests,
        report.search.lookup.fuzzy_lookup_terms,
        report.search.lookup.fuzzy_candidate_terms,
        report.search.lookup.fuzzy_verified_candidates,
        report.search.lookup.fuzzy_cache_hits,
        report.search.lookup.fuzzy_cache_misses,
        report.search.lookup.fuzzy_key_truncated_terms,
        report.search.lookup.fuzzy_term_truncated_keys,
        report.search.lookup.fuzzy_candidate_truncated_terms
    )
}

fn print_empty_sidecar_session_report(label: &str, budget: SearchLookupBudget) {
    eprintln!(
        "{label}\t{}\trecords-indexed=0\tcolumns-indexed=0\trecords-loaded=0\trecords-missing=0\tcandidate-ids=0\tfull-hydration=false\tmetadata-keys=0\tprefix-keys=0\tsubstring-keys=0\tfuzzy-keys=0\tcontent-keys=0\tcontent-cache-hits=0\tcontent-cache-misses=0\trecord-cache-hits=0\trecord-cache-misses=0\tresult-cache-hits=0\tresult-cache-misses=0\tmetadata-budget={}\tprefix-budget={}\tsubstring-budget={}\tfuzzy-key-budget={}\tfuzzy-term-budget={}\tfuzzy-candidate-budget={}\tcontent-budget={}\tprefix-archive-keys=0\tsubstring-archive-keys=0\tfuzzy-archive-keys=0\tprefix-lookup-requests=0\tprefix-lookup-ids=0\tprefix-cache-hits=0\tprefix-cache-misses=0\tsubstring-lookup-requests=0\tsubstring-lookup-ids=0\tsubstring-cache-hits=0\tsubstring-cache-misses=0\tfuzzy-lookup-requests=0\tfuzzy-lookup-terms=0\tfuzzy-cache-hits=0\tfuzzy-cache-misses=0",
        SidecarAdmittedVolumeSummary::empty().as_tsv_fields(),
        budget.max_metadata_ids_per_term,
        budget.max_prefix_ids_per_term,
        budget.max_substring_ids_per_gram,
        budget.max_fuzzy_keys_per_term,
        budget.max_fuzzy_terms_per_key,
        budget.max_fuzzy_candidates_per_term,
        budget.max_content_ids_per_term
    );
}

fn volume_scope_is_empty(scope: &SearchVolumeScope) -> bool {
    matches!(scope, SearchVolumeScope::Only(volumes) if volumes.is_empty())
}

fn parse_search_bool(value: &str, name: &str) -> Result<bool> {
    match value {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        other => Err(GfmError::Format(format!(
            "invalid {name} `{other}`, expected true or false"
        ))),
    }
}

fn highlight_snippet(snippet: &gfm_types::SearchSnippet) -> String {
    let Some(highlight) = snippet.highlights.first() else {
        return snippet.text.clone();
    };
    if highlight.start > highlight.end
        || highlight.end > snippet.text.len()
        || !snippet.text.is_char_boundary(highlight.start)
        || !snippet.text.is_char_boundary(highlight.end)
    {
        return snippet.text.clone();
    }
    format!(
        "{}[[{}]]{}",
        &snippet.text[..highlight.start],
        &snippet.text[highlight.start..highlight.end],
        &snippet.text[highlight.end..]
    )
}

fn escape_output_field(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            '\t' | '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

fn stream_stage(stage: SearchStreamStage) -> &'static str {
    match stage {
        SearchStreamStage::Hot => "hot",
        SearchStreamStage::Deep => "deep",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_search_paths_preserves_first_occurrence_order() {
        let first = PathBuf::from("/tmp/gfm-search-preflight/first.gfmcontent");
        let second = PathBuf::from("/tmp/gfm-search-preflight/second.gfmcontent");
        let paths = vec![
            first.clone(),
            second.clone(),
            first.clone(),
            second.clone(),
            first.clone(),
        ];

        let unique = unique_search_paths(&paths);

        assert_eq!(unique, vec![first.as_path(), second.as_path()]);
    }

    #[test]
    fn unique_sidecar_search_paths_preserves_first_role_for_repeated_paths() {
        let root = PathBuf::from("/tmp/gfm-sidecar-search-preflight");
        let shared = root.join("shared.gfmidx");
        let metadata = root.join("metadata.gfmmeta");
        let content = root.join("content.gfmcontent");
        let paths = SidecarIndexAccessPaths {
            records: &shared,
            columns: &shared,
            metadata: &metadata,
            prefixes: &shared,
            substrings: &metadata,
            fuzzy: &metadata,
            content: &content,
        };

        let unique = unique_sidecar_search_paths(paths.paths_with_roles())
            .into_iter()
            .map(|(path, role)| (path.to_path_buf(), role))
            .collect::<Vec<_>>();

        assert_eq!(
            unique,
            vec![
                (shared, "records"),
                (metadata, "metadata"),
                (content, "content"),
            ]
        );
    }

    #[test]
    fn sidecar_session_open_helper_passes_runtime_token_to_index_session_open() {
        let root = std::env::temp_dir().join(format!(
            "gfm-sidecar-session-open-token-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let unavailable = root.join(format!("{}.gfmidx", "records-unavailable".repeat(64)));
        let paths = OwnedSidecarIndexAccessPaths {
            records: unavailable,
            columns: root.join("columns.gfmcols"),
            metadata: root.join("metadata.gfmmeta"),
            prefixes: root.join("prefixes.gfmprefix"),
            substrings: root.join("substrings.gfmsubstr"),
            fuzzy: root.join("fuzzy.gfmfuzzy"),
            content: root.join("content.gfmcontent"),
        };
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = open_sidecar_index_query_session(paths, &cancellation);

        assert!(matches!(result, Err(GfmError::Cancelled)));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_archive_read_cancellable_passes_runtime_token_to_reader() {
        let path = std::env::temp_dir().join(format!(
            "gfm-search-archive-cancellation-token-{}.gfmidx",
            std::process::id()
        ));
        std::fs::write(&path, b"token-probe").unwrap();

        let result = run_search_archive_read_cancellable(
            path.clone(),
            "search archive cancellation token",
            |_path, cancellation| {
                cancellation.cancel();
                cancellation.check()
            },
        );

        assert_eq!(result, Err(GfmError::Cancelled));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn content_archive_read_cancellable_passes_runtime_token_to_reader() {
        let path = std::env::temp_dir().join(format!(
            "gfm-content-archive-cancellation-token-{}.gfmcontent",
            std::process::id()
        ));
        std::fs::write(&path, b"token-probe").unwrap();

        let result = run_content_archive_read_cancellable(
            path.clone(),
            "content archive cancellation token",
            |_path, cancellation| {
                cancellation.cancel();
                cancellation.check()
            },
        );

        assert_eq!(result, Err(GfmError::Cancelled));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn search_archive_access_checked_honors_pre_cancelled_control() {
        let path = std::env::temp_dir().join(format!(
            "gfm-search-archive-access-pre-cancel-{}.gfmidx",
            std::process::id()
        ));
        let volume_reports =
            ArchiveVolumeAccessReports::for_paths_checked([path.as_path()], || Ok(())).unwrap();

        let result = volume_reports
            .preflight_access_checked("search archive access", || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn search_archive_volume_reports_checked_honor_pre_cancelled_control_before_discovery() {
        let path = std::env::temp_dir()
            .join(format!(
                "gfm-search-archive-report-pre-cancel-{}",
                std::process::id()
            ))
            .join("records.gfmidx");

        let result = ArchiveVolumeAccessReports::for_paths_checked([path.as_path()], || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn search_root_access_report_checked_honors_pre_cancelled_control_before_discovery() {
        let root = std::env::temp_dir()
            .join(format!(
                "gfm-search-root-report-pre-cancel-{}",
                std::process::id()
            ))
            .join("root");

        let result = SearchRootAccessReport::new_checked(root.clone(), || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn search_retry_probe_report_checked_honors_pre_cancelled_control_before_probe() {
        let path = std::env::temp_dir()
            .join(format!(
                "gfm-search-retry-report-pre-cancel-{}",
                std::process::id()
            ))
            .join("retry.state");

        let result =
            search_retry_probe_access_report_checked(Some(&path), || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn search_retry_probe_report_refuses_unreachable_state_before_write_probe() {
        let root = std::env::temp_dir().join(format!(
            "gfm-search-retry-report-unreachable-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let path = root.join("retry-state-unavailable".repeat(16));

        let err = match search_retry_probe_access_report_checked(Some(&path), || Ok(())) {
            Ok(_) => panic!("unreachable search retry probe was admitted before volume preflight"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("search retry probe volume access blocked: unreachable volume network"),
            "{err}"
        );
        assert!(
            !err.to_string()
                .contains("search retry probe metadata unavailable"),
            "{err}"
        );
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_index_columns_access_checked_honors_pre_cancelled_control() {
        let root = std::env::temp_dir().join(format!(
            "gfm-search-index-columns-access-pre-cancel-{}",
            std::process::id()
        ));
        let records = root.join("records.gfmidx");
        let columns = root.join("columns.gfmcols");
        let volume_reports =
            SearchIndexColumnsVolumeAccessReports::for_paths_checked(&records, &columns, || Ok(()))
                .unwrap();

        let result = preflight_search_index_columns_access_checked(&volume_reports, || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn search_index_columns_reports_checked_can_cancel_between_inputs() {
        let root = std::env::temp_dir().join(format!(
            "gfm-search-index-columns-report-cancel-{}",
            std::process::id()
        ));
        let records = root.join("records.gfmidx");
        let columns = root.join("columns.gfmcols");
        let mut checks = 0;

        let result =
            SearchIndexColumnsVolumeAccessReports::for_paths_checked(&records, &columns, || {
                checks += 1;
                if checks > 3 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn content_index_manifest_access_checked_honors_pre_cancelled_control() {
        let root = std::env::temp_dir().join(format!(
            "gfm-content-index-manifest-access-pre-cancel-{}",
            std::process::id()
        ));
        let records = root.join("records.gfmidx");
        let manifest = root.join("content.gfmmanifest");
        let volume_reports =
            ContentIndexManifestVolumeAccessReports::for_records_and_manifest_checked(
                &records,
                &manifest,
                || Ok(()),
            )
            .unwrap();

        let result = preflight_content_index_manifest_search_access_checked(
            &manifest,
            &volume_reports,
            "content index manifest search",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn content_index_manifest_reports_checked_can_cancel_between_inputs() {
        let root = std::env::temp_dir().join(format!(
            "gfm-content-index-manifest-report-cancel-{}",
            std::process::id()
        ));
        let records = root.join("records.gfmidx");
        let manifest = root.join("content.gfmmanifest");
        let mut checks = 0;

        let result = ContentIndexManifestVolumeAccessReports::for_records_and_manifest_checked(
            &records,
            &manifest,
            || {
                checks += 1;
                if checks > 3 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn archive_volume_access_reports_honor_cancellation_between_paths() {
        let root = std::env::temp_dir().join(format!(
            "gfm-content-archives-access-cancel-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.gfmcontent");
        let second = root.join("second.gfmcontent");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let mut checks = 0usize;

        let result = ArchiveVolumeAccessReports::for_paths_checked(
            [first.as_path(), second.as_path()],
            || {
                checks += 1;
                if checks >= 4 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 4);
        assert!(first.exists());
        assert!(second.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn content_index_access_checked_honors_pre_cancelled_control() {
        let root = std::env::temp_dir().join(format!(
            "gfm-content-index-access-pre-cancel-{}",
            std::process::id()
        ));
        let records = root.join("records.gfmidx");
        let content_paths = vec![root.join("content.gfmcontent")];
        let volume_reports =
            ContentIndexVolumeAccessReports::for_records_and_content_paths_checked(
                &records,
                &content_paths,
                || Ok(()),
            )
            .unwrap();

        let result = preflight_content_index_search_access_checked(
            &volume_reports,
            "content index access",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn content_index_reports_checked_can_cancel_between_content_paths() {
        let root = std::env::temp_dir().join(format!(
            "gfm-content-index-report-cancel-{}",
            std::process::id()
        ));
        let records = root.join("records.gfmidx");
        let content_paths = vec![
            root.join("first.gfmcontent"),
            root.join("second.gfmcontent"),
        ];
        let mut checks = 0;

        let result = ContentIndexVolumeAccessReports::for_records_and_content_paths_checked(
            &records,
            &content_paths,
            || {
                checks += 1;
                if checks > 5 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn sidecar_index_access_checked_honors_pre_cancelled_control() {
        let root = std::env::temp_dir().join(format!(
            "gfm-sidecar-access-pre-cancel-{}",
            std::process::id()
        ));
        let paths = OwnedSidecarIndexAccessPaths {
            records: root.join("records.gfmidx"),
            columns: root.join("columns.gfmcols"),
            metadata: root.join("metadata.gfmmeta"),
            prefixes: root.join("prefixes.gfmprefix"),
            substrings: root.join("substrings.gfmsubstr"),
            fuzzy: root.join("fuzzy.gfmfuzzy"),
            content: root.join("content.gfmcontent"),
        };

        let volume_reports =
            SidecarVolumeAccessReports::for_paths_checked(paths.borrowed(), || Ok(())).unwrap();
        let result = preflight_sidecar_index_search_access_checked(
            &volume_reports,
            "sidecar index access",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }

    #[test]
    fn sidecar_index_reports_checked_can_cancel_between_unique_paths() {
        let root =
            std::env::temp_dir().join(format!("gfm-sidecar-report-cancel-{}", std::process::id()));
        let paths = OwnedSidecarIndexAccessPaths {
            records: root.join("records.gfmidx"),
            columns: root.join("records.gfmidx"),
            metadata: root.join("metadata.gfmmeta"),
            prefixes: root.join("records.gfmidx"),
            substrings: root.join("metadata.gfmmeta"),
            fuzzy: root.join("metadata.gfmmeta"),
            content: root.join("content.gfmcontent"),
        };
        let mut checks = 0;

        let result = SidecarVolumeAccessReports::for_paths_checked(paths.borrowed(), || {
            checks += 1;
            if checks > 5 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }
}
