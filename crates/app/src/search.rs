use crate::access::{
    preflight_access_scope_checked, preflight_access_scope_checked_with_volume_report,
    preflight_volume_access_scope, preflight_volume_access_scope_with_report, ScopedAccessGuard,
};
use crate::content::run_content_search;
use crate::extract::extraction_budget_profile;
use crate::runtime::run_retriable_volume_task_cancellable_with_payload_path;
use crate::{
    first_path_volume, parse_required_scheduling_pressure, parse_usize_arg, path_volume,
    required_path, required_string,
};
use gfm_content::Extractor;
use gfm_index::{
    Indexer, LiveIndex, SearchLookupBudget, SearchMetadataPosting, SearchPrefixPosting,
    SearchQuery, SearchRecordColumns, SearchStreamStage, SearchVolumeScope,
    SidecarIndexQuerySession, SidecarQueryImport, SidecarQuerySessionReport,
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
use std::collections::BTreeSet;
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
            preflight_volume_access_scope(&root, AccessIntent::Index, "search")?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                preflight_volume_access_scope(
                    write_probe_path(retry_probe)?,
                    AccessIntent::Write,
                    "search",
                )?;
            }
            let volume = path_volume(&root);
            let hits = run_retriable_volume_task_cancellable_with_payload_path(
                volume,
                Priority::Visible,
                "search",
                root.clone(),
                move |cancellation| {
                    let root = root.clone();
                    let query = query.clone();
                    let retry_probe = retry_probe.clone();
                    cancellation.check()?;
                    if let Some(retry_probe) = retry_probe.as_ref() {
                        fail_first_search_retry_probe_attempt(
                            retry_probe,
                            "search",
                            &cancellation,
                        )?;
                    }
                    let _access = preflight_access_scope_checked(
                        &root,
                        AccessIntent::Index,
                        "search",
                        || cancellation.check(),
                    )?;
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
            preflight_volume_access_scope(&root, AccessIntent::Index, "search stream")?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                preflight_volume_access_scope(
                    write_probe_path(retry_probe)?,
                    AccessIntent::Write,
                    "search stream",
                )?;
            }
            let volume = path_volume(&root);
            let batches = run_retriable_volume_task_cancellable_with_payload_path(
                volume,
                Priority::Visible,
                "search stream",
                root.clone(),
                move |cancellation| {
                    let root = root.clone();
                    let query = query.clone();
                    let retry_probe = retry_probe.clone();
                    cancellation.check()?;
                    if let Some(retry_probe) = retry_probe.as_ref() {
                        fail_first_search_retry_probe_attempt(
                            retry_probe,
                            "search stream",
                            &cancellation,
                        )?;
                    }
                    let _access = preflight_access_scope_checked(
                        &root,
                        AccessIntent::Index,
                        "search stream",
                        || cancellation.check(),
                    )?;
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
            let extractor =
                Extractor::with_budget_profile(extraction_budget_profile(&root, pressure));
            let (indexed, hits) = run_content_search(root, query, extractor)?;
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
            let root = records
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let extractor =
                Extractor::with_budget_profile(extraction_budget_profile(&root, pressure));
            let output = run_content_index_search(
                records,
                content,
                query,
                extractor,
                "adaptive content index search",
                None,
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

fn preflight_content_archive_access_checked(
    path: &Path,
    worker: &str,
    check_control: impl FnMut() -> Result<()>,
) -> Result<ScopedAccessGuard> {
    preflight_access_scope_checked(path, AccessIntent::Read, worker, check_control)
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
    preflight_volume_access_scope(&path, AccessIntent::Read, worker)?;
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, worker)?;
    }
    let volume = path_volume(&path);
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        worker,
        path.clone(),
        move |cancellation| {
            let path = path.clone();
            let retry_probe = retry_probe.clone();
            let read = read.clone();
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, worker, &cancellation)?;
            }
            let _access =
                preflight_content_archive_access_checked(&path, worker, || cancellation.check())?;
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
    preflight_content_archive_volumes(&paths, worker)?;
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, worker)?;
    }
    let volume = first_path_volume(paths.iter());
    let payload_path = paths.first().cloned().unwrap_or_else(|| PathBuf::from("."));
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        worker,
        payload_path,
        move |cancellation| {
            let paths = paths.clone();
            let retry_probe = retry_probe.clone();
            let read = read.clone();
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, worker, &cancellation)?;
            }
            let _access =
                preflight_content_archives_access_checked(&paths, worker, || cancellation.check())?;
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
    preflight_volume_access_scope(&manifest, AccessIntent::Read, worker)?;
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, worker)?;
    }
    let volume = path_volume(&manifest);
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        worker,
        manifest.clone(),
        move |cancellation| {
            let manifest = manifest.clone();
            let retry_probe = retry_probe.clone();
            let read = read.clone();
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, worker, &cancellation)?;
            }
            let _access = preflight_content_manifest_access_checked(&manifest, worker, || {
                cancellation.check()
            })?;
            cancellation.check()?;
            read(manifest, &cancellation)
        },
    )
}

fn preflight_content_archive_volumes(paths: &[PathBuf], worker: &str) -> Result<()> {
    for path in paths {
        preflight_volume_access_scope(path, AccessIntent::Read, worker)?;
    }
    Ok(())
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
    preflight_volume_access_scope(&path, AccessIntent::Read, worker)?;
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, worker)?;
    }
    let volume = path_volume(&path);
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        worker,
        path.clone(),
        move |cancellation| {
            let path = path.clone();
            let retry_probe = retry_probe.clone();
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, worker, &cancellation)?;
            }
            let _access =
                preflight_search_archive_access_checked(&path, worker, || cancellation.check())?;
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

fn fail_first_search_retry_probe_attempt(
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
    preflight_volume_access_scope(&records, AccessIntent::Read, "search index columns records")?;
    preflight_volume_access_scope(&columns, AccessIntent::Read, "search index columns columns")?;
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, WORKER)?;
    }
    let volume = path_volume(&records).or_else(|| path_volume(&columns));
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
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, WORKER, &cancellation)?;
            }
            let _access =
                preflight_search_index_columns_access_checked(&records, &columns, || {
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
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, worker)?;
    }
    let volume = volume_reports.first_volume();
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
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, worker, &cancellation)?;
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
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, WORKER)?;
    }
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
            let retry_probe = retry_probe.clone();
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, WORKER, &cancellation)?;
            }
            let _access =
                preflight_content_index_set_search_access_checked(&volume_reports, WORKER, || {
                    cancellation.check()
                })?;
            cancellation.check()?;
            let archive_count = content_paths.len();
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
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, WORKER)?;
    }
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
            let retry_probe = retry_probe.clone();
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, WORKER, &cancellation)?;
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
            let parsed = SearchQuery::parse_cancellable(&query, &cancellation)?;
            let first = session.search_structured_with_budget_cancellable(
                &parsed,
                50,
                SearchLookupBudget::default(),
                &cancellation,
            )?;
            let mut diagnostics = vec![format_content_session_report(
                "content-session-first",
                content_paths.len(),
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
                content_paths.len(),
                &second,
            ));
            hits.extend(second.search.hits);
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
    preflight_volume_access_scope(&records, AccessIntent::Read, &format!("{WORKER} records"))?;
    preflight_volume_access_scope(&manifest, AccessIntent::Read, &format!("{WORKER} manifest"))?;
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, WORKER)?;
    }
    let volume = path_volume(&records).or_else(|| path_volume(&manifest));
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
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, WORKER, &cancellation)?;
            }
            let _access = preflight_content_index_manifest_search_access_checked(
                &records,
                &manifest,
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
    preflight_volume_access_scope(&records, AccessIntent::Read, &format!("{WORKER} records"))?;
    preflight_volume_access_scope(&manifest, AccessIntent::Read, &format!("{WORKER} manifest"))?;
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, WORKER)?;
    }
    let volume = path_volume(&records).or_else(|| path_volume(&manifest));
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
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, WORKER, &cancellation)?;
            }
            let _access = preflight_content_index_manifest_search_access_checked(
                &records,
                &manifest,
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
    let reports =
        ContentIndexVolumeAccessReports::for_records_and_content_paths(records, content_paths);
    reports.preflight_volumes(worker)?;
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
    fn for_records_and_content_paths(records: &Path, content_paths: &[PathBuf]) -> Self {
        let mut entries = vec![ContentIndexVolumeAccessReport {
            path: records.to_path_buf(),
            role: "records",
            volume_report: VolumeDiscoveryReport::for_containing_path(records),
        }];
        entries.extend(unique_search_paths(content_paths).into_iter().map(|path| {
            ContentIndexVolumeAccessReport {
                path: path.to_path_buf(),
                role: "content",
                volume_report: VolumeDiscoveryReport::for_containing_path(path),
            }
        }));
        Self { entries }
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

    fn first_volume(&self) -> Option<VolumeId> {
        self.entries.iter().find_map(|entry| {
            entry
                .volume_report
                .volume_for_path(&entry.path)
                .map(|volume| volume.id)
        })
    }
}

fn preflight_search_archive_access_checked(
    path: &Path,
    worker: &str,
    check_control: impl FnMut() -> Result<()>,
) -> Result<ScopedAccessGuard> {
    preflight_access_scope_checked(path, AccessIntent::Read, worker, check_control)
}

fn preflight_search_index_columns_access_checked(
    records: &Path,
    columns: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope_checked(
            records,
            AccessIntent::Read,
            "search index columns records",
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            columns,
            AccessIntent::Read,
            "search index columns columns",
            &mut check_control,
        )?,
    ])
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
    records: &Path,
    manifest_path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let mut guards = vec![
        preflight_access_scope_checked(
            records,
            AccessIntent::Read,
            &format!("{worker} records"),
            &mut check_control,
        )?,
        preflight_access_scope_checked(
            manifest_path,
            AccessIntent::Read,
            &format!("{worker} manifest"),
            &mut check_control,
        )?,
    ];
    check_control()?;
    let manifest = ContentArchiveManifest::read_checked(manifest_path, &mut check_control)?;
    check_control()?;
    let content_worker = format!("{worker} content");
    guards.extend(preflight_content_archives_access_checked(
        &manifest.resolved_archive_paths(manifest_path),
        &content_worker,
        &mut check_control,
    )?);
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
    fn for_paths(paths: SidecarIndexAccessPaths<'_>) -> Self {
        let entries = unique_sidecar_search_paths(paths.paths_with_roles())
            .into_iter()
            .map(|(path, role)| SidecarVolumeAccessReport {
                path: path.to_path_buf(),
                role,
                volume_report: VolumeDiscoveryReport::for_containing_path(path),
            })
            .collect();
        Self { entries }
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

    fn first_volume(&self) -> Option<VolumeId> {
        self.entries.iter().find_map(|entry| {
            entry
                .volume_report
                .volume_for_path(&entry.path)
                .map(|volume| volume.id)
        })
    }
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
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, WORKER)?;
    }
    let volume = volume_reports.first_volume();
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        paths.records.clone(),
        move |cancellation| {
            let paths = paths.clone();
            let query = query.clone();
            let retry_probe = retry_probe.clone();
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, WORKER, &cancellation)?;
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
            let diagnostics = format!(
            "columns-indexed {} records-loaded {} records-missing {} candidate-ids {} full-hydration {} metadata-keys {} prefix-keys {} substring-keys {} fuzzy-keys {} prefix-archive-keys {} substring-archive-keys {} fuzzy-archive-keys {} content-keys {} content-cache-hits {} content-cache-misses {} metadata-budget {} substring-budget {} content-budget {}",
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
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, WORKER)?;
    }
    let volume = volume_reports.first_volume();
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        paths.records.clone(),
        move |cancellation| {
            let paths = paths.clone();
            let query = query.clone();
            let retry_probe = retry_probe.clone();
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, WORKER, &cancellation)?;
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
                        budget,
                    ),
                    format_sidecar_session_report(
                        "sidecar-session-second",
                        &session,
                        &second,
                        budget,
                    ),
                ],
                hits: second.search.hits,
            })
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
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, WORKER)?;
    }
    let volume = volume_reports.first_volume();
    run_retriable_volume_task_cancellable_with_payload_path(
        volume,
        Priority::Visible,
        WORKER,
        paths.records.clone(),
        move |cancellation| {
            let paths = paths.clone();
            let query = query.clone();
            let retry_probe = retry_probe.clone();
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, WORKER, &cancellation)?;
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
                diagnostics: format_sidecar_budget_report(&session, &report, budget),
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
    if let Some(retry_probe) = retry_probe.as_ref() {
        preflight_volume_access_scope(write_probe_path(retry_probe)?, AccessIntent::Write, WORKER)?;
    }
    let volume = volume_reports.first_volume();
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
            cancellation.check()?;
            if let Some(retry_probe) = retry_probe.as_ref() {
                fail_first_search_retry_probe_attempt(retry_probe, WORKER, &cancellation)?;
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
    let reports = SidecarVolumeAccessReports::for_paths(paths.borrowed());
    reports.preflight_volumes(worker)?;
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

fn preflight_content_archives_access_checked(
    paths: &[PathBuf],
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    unique_search_paths(paths)
        .into_iter()
        .map(|path| preflight_content_archive_access_checked(path, worker, &mut check_control))
        .collect()
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

fn preflight_content_manifest_access_checked(
    manifest_path: &Path,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let mut guards = vec![preflight_content_archive_access_checked(
        manifest_path,
        worker,
        &mut check_control,
    )?];
    check_control()?;
    let manifest = ContentArchiveManifest::read_checked(manifest_path, &mut check_control)?;
    check_control()?;
    guards.extend(preflight_content_archives_access_checked(
        &manifest.resolved_archive_paths(manifest_path),
        worker,
        &mut check_control,
    )?);
    check_control()?;
    Ok(guards)
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
    budget: SearchLookupBudget,
) -> String {
    let hydration = &report.hydration;
    format!(
        "{label}\trecords-indexed={}\tcolumns-indexed={}\trecords-loaded={}\trecords-missing={}\tcandidate-ids={}\tfull-hydration={}\tmetadata-keys={}\tprefix-keys={}\tsubstring-keys={}\tfuzzy-keys={}\tcontent-keys={}\tcontent-cache-hits={}\tcontent-cache-misses={}\trecord-cache-hits={}\trecord-cache-misses={}\tresult-cache-hits={}\tresult-cache-misses={}\tmetadata-budget={}\tprefix-budget={}\tsubstring-budget={}\tfuzzy-key-budget={}\tfuzzy-term-budget={}\tfuzzy-candidate-budget={}\tcontent-budget={}\tprefix-archive-keys={}\tsubstring-archive-keys={}\tfuzzy-archive-keys={}\tprefix-lookup-requests={}\tprefix-lookup-ids={}\tprefix-cache-hits={}\tprefix-cache-misses={}\tsubstring-lookup-requests={}\tsubstring-lookup-ids={}\tsubstring-cache-hits={}\tsubstring-cache-misses={}\tfuzzy-lookup-requests={}\tfuzzy-lookup-terms={}\tfuzzy-cache-hits={}\tfuzzy-cache-misses={}",
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
    budget: SearchLookupBudget,
) -> String {
    let hydration = &report.hydration;
    format!(
        "sidecar-budget\tcolumns-indexed={}\trecords-loaded={}\trecords-missing={}\tcandidate-ids={}\tfull-hydration={}\tmetadata-keys={}\tprefix-keys={}\tsubstring-keys={}\tfuzzy-keys={}\tcontent-keys={}\tcontent-cache-hits={}\tcontent-cache-misses={}\tmetadata-budget={}\tcontent-budget={}\tprefix-archive-keys={}\tsubstring-archive-keys={}\tfuzzy-archive-keys={}\tprefix-terms={}\tprefix-lookup-requests={}\tprefix-lookup-ids={}\tprefix-candidate-ids={}\tprefix-cache-hits={}\tprefix-cache-misses={}\tprefix-cutoff-terms={}\tprefix-truncated-terms={}\tsubstring-terms={}\tsubstring-grams={}\tsubstring-lookup-requests={}\tsubstring-lookup-ids={}\tsubstring-candidate-ids={}\tsubstring-cache-hits={}\tsubstring-cache-misses={}\tsubstring-cutoff-terms={}\tsubstring-term-truncated-grams={}\tsubstring-truncated-grams={}\tfuzzy-terms={}\tfuzzy-keys-read={}\tfuzzy-lookup-requests={}\tfuzzy-lookup-terms={}\tfuzzy-candidate-terms={}\tfuzzy-verified-candidates={}\tfuzzy-cache-hits={}\tfuzzy-cache-misses={}\tfuzzy-key-truncated-terms={}\tfuzzy-term-truncated-keys={}\tfuzzy-candidate-truncated-terms={}",
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
        "{label}\trecords-indexed=0\tcolumns-indexed=0\trecords-loaded=0\trecords-missing=0\tcandidate-ids=0\tfull-hydration=false\tmetadata-keys=0\tprefix-keys=0\tsubstring-keys=0\tfuzzy-keys=0\tcontent-keys=0\tcontent-cache-hits=0\tcontent-cache-misses=0\trecord-cache-hits=0\trecord-cache-misses=0\tresult-cache-hits=0\tresult-cache-misses=0\tmetadata-budget={}\tprefix-budget={}\tsubstring-budget={}\tfuzzy-key-budget={}\tfuzzy-term-budget={}\tfuzzy-candidate-budget={}\tcontent-budget={}\tprefix-archive-keys=0\tsubstring-archive-keys=0\tfuzzy-archive-keys=0\tprefix-lookup-requests=0\tprefix-lookup-ids=0\tprefix-cache-hits=0\tprefix-cache-misses=0\tsubstring-lookup-requests=0\tsubstring-lookup-ids=0\tsubstring-cache-hits=0\tsubstring-cache-misses=0\tfuzzy-lookup-requests=0\tfuzzy-lookup-terms=0\tfuzzy-cache-hits=0\tfuzzy-cache-misses=0",
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

        let result =
            preflight_search_archive_access_checked(&path, "search archive access", || {
                Err(GfmError::Cancelled)
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn content_archives_access_checked_honors_cancellation_between_paths() {
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

        let result = preflight_content_archives_access_checked(
            &[first.clone(), second.clone()],
            "content archives access",
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
        let volume_reports = ContentIndexVolumeAccessReports::for_records_and_content_paths(
            &records,
            &content_paths,
        );

        let result = preflight_content_index_search_access_checked(
            &volume_reports,
            "content index access",
            || Err(GfmError::Cancelled),
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

        let volume_reports = SidecarVolumeAccessReports::for_paths(paths.borrowed());
        let result = preflight_sidecar_index_search_access_checked(
            &volume_reports,
            "sidecar index access",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!root.exists());
    }
}
