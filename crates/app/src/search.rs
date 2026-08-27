use crate::access::{preflight_access_scope, ScopedAccessGuard};
use crate::content::run_content_search;
use crate::extract::extraction_budget_profile;
use crate::{parse_required_scheduling_pressure, parse_usize_arg, required_path, required_string};
use gfm_content::Extractor;
use gfm_index::{
    Indexer, LiveIndex, SearchLookupBudget, SearchRecordColumns, SearchStreamStage,
    SearchVolumeScope, SidecarIndexQuerySession, SidecarQuerySessionReport,
};
use gfm_mac::AccessIntent;
use gfm_store::{
    ContentArchive, MetadataField, MmapContentArchive, MmapContentSet, MmapDictionary,
    MmapFuzzyArchive, MmapMetadataArchive, MmapPrefixArchive, MmapRecordArchive, MmapRecordColumns,
    MmapSubstringArchive,
};
use gfm_types::{FileKind, GfmError, Result, SearchHit, VolumeId};
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "search" => {
            let root = required_path(args.next(), "search requires a root path")?;
            let query = required_string(args.next(), "search requires a query string")?;
            let _access = preflight_access_scope(&root, AccessIntent::Index, "search")?;
            let snapshot = Indexer::default().build(root)?;
            let session = snapshot.query_session();
            for hit in session.search(&query, 50) {
                print_hit(&hit);
            }
        }
        "search-stream" => {
            let root = required_path(args.next(), "search-stream requires a root path")?;
            let query = required_string(args.next(), "search-stream requires a query string")?;
            let _access = preflight_access_scope(&root, AccessIntent::Index, "search stream")?;
            let snapshot = Indexer::default().build(root)?;
            let session = snapshot.query_session();
            for batch in session.stream_search(&query, 50)? {
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
        "search-content-index" => {
            let records =
                required_path(args.next(), "search-content-index requires a records path")?;
            let content =
                required_path(args.next(), "search-content-index requires a content path")?;
            let query =
                required_string(args.next(), "search-content-index requires a query string")?;
            let _access =
                preflight_content_index_search_access(&records, &content, "content index search")?;
            let (live, report) =
                Indexer::default().load_live_with_content_for_query(records, content, &query)?;
            eprintln!(
                "content-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
                report.content_keys,
                report.records_loaded,
                report.records_missing,
                report.candidate_ids,
                report.full_hydration
            );
            for hit in live.search_with_snippets(&query, 50, &Extractor::default(), 96)? {
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
            let _access = preflight_content_index_search_access(
                &records,
                &content,
                "adaptive content index search",
            )?;
            let (live, report) =
                Indexer::default().load_live_with_content_for_query(records, content, &query)?;
            eprintln!(
                "content-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
                report.content_keys,
                report.records_loaded,
                report.records_missing,
                report.candidate_ids,
                report.full_hydration
            );
            for hit in live.search_with_snippets(&query, 50, &extractor, 96)? {
                print_hit(&hit);
            }
        }
        "search-content-index-set" => {
            let records = required_path(
                args.next(),
                "search-content-index-set requires a records path",
            )?;
            let query = required_string(
                args.next(),
                "search-content-index-set requires a query string",
            )?;
            let content_paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if content_paths.is_empty() {
                return Err(gfm_types::GfmError::Format(
                    "search-content-index-set requires at least one content archive".to_string(),
                ));
            }
            let _access = preflight_content_index_set_search_access(
                &records,
                &content_paths,
                "content index set search",
            )?;
            let (live, report) =
                Indexer::default().load_live_with_content_set(records, &content_paths, &query)?;
            eprintln!(
                "content-archives {} content-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
                content_paths.len(),
                report.content_keys,
                report.records_loaded,
                report.records_missing,
                report.candidate_ids,
                report.full_hydration
            );
            for hit in live.search(&query, 50) {
                print_hit(&hit);
            }
        }
        "search-content-index-set-session" => {
            let records = required_path(
                args.next(),
                "search-content-index-set-session requires a records path",
            )?;
            let query = required_string(
                args.next(),
                "search-content-index-set-session requires a query string",
            )?;
            let content_paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if content_paths.is_empty() {
                return Err(gfm_types::GfmError::Format(
                    "search-content-index-set-session requires at least one content archive"
                        .to_string(),
                ));
            }
            let _access = preflight_content_index_set_search_access(
                &records,
                &content_paths,
                "content index set session",
            )?;
            let session =
                Indexer::default().load_content_set_query_session(&records, &content_paths)?;
            let first = session.search(&query, 50)?;
            print_content_session_report("content-session-first", content_paths.len(), &first);
            for hit in first.search.hits {
                print_hit(&hit);
            }
            let second = session.search(&query, 50)?;
            print_content_session_report("content-session-second", content_paths.len(), &second);
            for hit in second.search.hits {
                print_hit(&hit);
            }
        }
        "search-content-index-manifest" => {
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
            let _access = preflight_content_index_search_access(
                &records,
                &manifest,
                "content index manifest search",
            )?;
            let (live, report) =
                Indexer::default().load_live_with_content_manifest(records, manifest, &query)?;
            eprintln!(
                "content-manifest-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
                report.content_keys,
                report.records_loaded,
                report.records_missing,
                report.candidate_ids,
                report.full_hydration
            );
            for hit in live.search(&query, 50) {
                print_hit(&hit);
            }
        }
        "search-content-index-manifest-session" => {
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
            let _access = preflight_content_index_search_access(
                &records,
                &manifest,
                "content index manifest session",
            )?;
            let session =
                Indexer::default().load_content_manifest_query_session(&records, &manifest)?;
            let first = session.search(&query, 50)?;
            print_content_session_report(
                "content-manifest-session-first",
                session.archive_count(),
                &first,
            );
            for hit in first.search.hits {
                print_hit(&hit);
            }
            let second = session.search(&query, 50)?;
            print_content_session_report(
                "content-manifest-session-second",
                session.archive_count(),
                &second,
            );
            for hit in second.search.hits {
                print_hit(&hit);
            }
        }
        "search-index" => {
            let index_path = required_path(args.next(), "search-index requires an index path")?;
            let query = required_string(args.next(), "search-index requires a query string")?;
            let session = Indexer::default().load_query_session(index_path)?;
            for hit in session.search(&query, 50) {
                print_hit(&hit);
            }
        }
        "search-index-mmap" => {
            let index_path =
                required_path(args.next(), "search-index-mmap requires an index path")?;
            let query = required_string(args.next(), "search-index-mmap requires a query string")?;
            let _access = preflight_search_archive_access(&index_path, "search index mmap")?;
            let live = LiveIndex::from_records(MmapRecordArchive::open(index_path)?.records()?);
            for hit in live.search(&query, 50) {
                print_hit(&hit);
            }
        }
        "search-index-columns" => {
            let records =
                required_path(args.next(), "search-index-columns requires a records path")?;
            let columns =
                required_path(args.next(), "search-index-columns requires a columns path")?;
            let query =
                required_string(args.next(), "search-index-columns requires a query string")?;
            let _access = preflight_search_index_columns_access(&records, &columns)?;
            let records = MmapRecordArchive::open(records)?;
            let columns = MmapRecordColumns::open(columns)?;
            let mut search_columns = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
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
            let (live, applied) =
                LiveIndex::from_records_with_columns(records.records()?, search_columns);
            eprintln!("columns-indexed {applied}");
            for hit in live.search(&query, 50) {
                print_hit(&hit);
            }
        }
        "search-index-sidecars" => {
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
            let sidecars = SidecarIndexAccessPaths {
                records: &records,
                columns: &columns,
                metadata: &metadata,
                prefixes: &prefixes,
                substrings: &substrings,
                fuzzy: &fuzzy,
                content: &content,
            };
            let _access = preflight_sidecar_index_search_access(sidecars, "sidecar search")?;
            let session = SidecarIndexQuerySession::open(
                records, columns, metadata, prefixes, substrings, fuzzy, content,
            )?;
            let budget = SearchLookupBudget::default();
            let report = session.search_with_budget(&query, 50, budget)?;
            let hydration = &report.hydration;
            eprintln!(
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
            for hit in report.search.hits {
                print_hit(&hit);
            }
        }
        "search-index-sidecars-session" => {
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
            let sidecars = SidecarIndexAccessPaths {
                records: &records,
                columns: &columns,
                metadata: &metadata,
                prefixes: &prefixes,
                substrings: &substrings,
                fuzzy: &fuzzy,
                content: &content,
            };
            let _access = preflight_sidecar_index_search_access(sidecars, "sidecar session")?;
            let session = SidecarIndexQuerySession::open(
                records, columns, metadata, prefixes, substrings, fuzzy, content,
            )?;
            let budget = SearchLookupBudget::default();
            let first = session.search_with_budget(&query, 50, budget)?;
            print_sidecar_session_report("sidecar-session-first", &session, &first, budget);
            let second = session.search_with_budget(&query, 50, budget)?;
            print_sidecar_session_report("sidecar-session-second", &session, &second, budget);
            for hit in second.search.hits {
                print_hit(&hit);
            }
        }
        "search-index-sidecars-budget" => {
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
            let sidecars = SidecarIndexAccessPaths {
                records: &records,
                columns: &columns,
                metadata: &metadata,
                prefixes: &prefixes,
                substrings: &substrings,
                fuzzy: &fuzzy,
                content: &content,
            };
            let _access = preflight_sidecar_index_search_access(sidecars, "sidecar budget")?;
            let session = SidecarIndexQuerySession::open(
                records, columns, metadata, prefixes, substrings, fuzzy, content,
            )?;
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
            let report = session.search_with_budget(&query, 50, budget)?;
            let hydration = &report.hydration;
            eprintln!(
                "sidecar-budget\tcolumns-indexed={}\trecords-loaded={}\trecords-missing={}\tcandidate-ids={}\tfull-hydration={}\tmetadata-keys={}\tprefix-keys={}\tsubstring-keys={}\tfuzzy-keys={}\tcontent-keys={}\tcontent-cache-hits={}\tcontent-cache-misses={}\tmetadata-budget={max_content_ids}\tcontent-budget={max_content_ids}\tprefix-archive-keys={}\tsubstring-archive-keys={}\tfuzzy-archive-keys={}\tprefix-terms={}\tprefix-lookup-requests={}\tprefix-lookup-ids={}\tprefix-candidate-ids={}\tprefix-cache-hits={}\tprefix-cache-misses={}\tprefix-cutoff-terms={}\tprefix-truncated-terms={}\tsubstring-terms={}\tsubstring-grams={}\tsubstring-lookup-requests={}\tsubstring-lookup-ids={}\tsubstring-candidate-ids={}\tsubstring-cache-hits={}\tsubstring-cache-misses={}\tsubstring-cutoff-terms={}\tsubstring-term-truncated-grams={}\tsubstring-truncated-grams={}\tfuzzy-terms={}\tfuzzy-keys-read={}\tfuzzy-lookup-requests={}\tfuzzy-lookup-terms={}\tfuzzy-candidate-terms={}\tfuzzy-verified-candidates={}\tfuzzy-cache-hits={}\tfuzzy-cache-misses={}\tfuzzy-key-truncated-terms={}\tfuzzy-term-truncated-keys={}\tfuzzy-candidate-truncated-terms={}",
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
            );
            for hit in report.search.hits {
                print_hit(&hit);
            }
        }
        "search-index-sidecars-volume-scope" => {
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
            let sidecars = SidecarIndexAccessPaths {
                records: &records,
                columns: &columns,
                metadata: &metadata,
                prefixes: &prefixes,
                substrings: &substrings,
                fuzzy: &fuzzy,
                content: &content,
            };
            let _access = preflight_sidecar_index_search_access(sidecars, "sidecar volume scope")?;
            let session = SidecarIndexQuerySession::open(
                records, columns, metadata, prefixes, substrings, fuzzy, content,
            )?;
            let budget = SearchLookupBudget::default();
            let report = session.search_with_volume_scope(&query, 50, &scope)?;
            print_sidecar_session_report("sidecar-volume-scope", &session, &report, budget);
            for hit in report.search.hits {
                print_hit(&hit);
            }
        }
        "content-ids" => {
            let content = required_path(args.next(), "content-ids requires a content path")?;
            let term = required_string(args.next(), "content-ids requires a term")?;
            let _access = preflight_content_archive_access(&content, "content ids")?;
            let mut archive = ContentArchive::open(content)?;
            print_file_ids(archive.ids_for_term(&term)?);
        }
        "content-ids-mmap" => {
            let content = required_path(args.next(), "content-ids-mmap requires a content path")?;
            let term = required_string(args.next(), "content-ids-mmap requires a term")?;
            let _access = preflight_content_archive_access(&content, "content ids mmap")?;
            let archive = MmapContentArchive::open(content)?;
            print_file_ids(archive.ids_for_term(&term)?);
        }
        "content-ids-mmap-set" => {
            let term = required_string(args.next(), "content-ids-mmap-set requires a term")?;
            let content_paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if content_paths.is_empty() {
                return Err(gfm_types::GfmError::Format(
                    "content-ids-mmap-set requires at least one content archive".to_string(),
                ));
            }
            let _access =
                preflight_content_archives_access(&content_paths, "content ids mmap set")?;
            let archive = MmapContentSet::open(&content_paths)?;
            print_file_ids(archive.ids_for_term(&term)?);
        }
        "content-ids-mmap-manifest" => {
            let manifest = required_path(
                args.next(),
                "content-ids-mmap-manifest requires a manifest path",
            )?;
            let term = required_string(args.next(), "content-ids-mmap-manifest requires a term")?;
            let _access = preflight_content_archive_access(&manifest, "content ids mmap manifest")?;
            let archive = MmapContentSet::open_manifest(manifest)?;
            print_file_ids(archive.ids_for_term(&term)?);
        }
        "content-id-block-mmap" => {
            let content =
                required_path(args.next(), "content-id-block-mmap requires a content path")?;
            let term = required_string(args.next(), "content-id-block-mmap requires a term")?;
            let block_index =
                parse_usize_arg(args.next(), "content-id-block-mmap requires a block index")?;
            let _access = preflight_content_archive_access(&content, "content id block mmap")?;
            let archive = MmapContentArchive::open(content)?;
            print_file_ids(archive.id_block_for_term(&term, block_index)?);
        }
        "content-verify" => {
            let content = required_path(args.next(), "content-verify requires a content path")?;
            let _access = preflight_content_archive_access(&content, "content verify")?;
            let archive = MmapContentArchive::open(content)?;
            println!(
                "content-verify\tterms={}\tbytes={}\tchecksum={}",
                archive.indexed_terms(),
                archive.mapped_len(),
                if archive.is_checksummed() {
                    "verified"
                } else {
                    "legacy"
                }
            );
        }
        "fuzzy-terms-mmap" => {
            let fuzzy = required_path(args.next(), "fuzzy-terms-mmap requires a fuzzy path")?;
            let key = required_string(args.next(), "fuzzy-terms-mmap requires a key")?;
            let _access = preflight_search_archive_access(&fuzzy, "fuzzy terms mmap")?;
            let archive = MmapFuzzyArchive::open(fuzzy)?;
            for term in archive.terms_for(&key)? {
                println!("{term}");
            }
        }
        "fuzzy-verify" => {
            let fuzzy = required_path(args.next(), "fuzzy-verify requires a fuzzy path")?;
            let _access = preflight_search_archive_access(&fuzzy, "fuzzy verify")?;
            let archive = MmapFuzzyArchive::open(fuzzy)?;
            println!(
                "fuzzy-verify\tkeys={}\tbytes={}\tchecksum={}",
                archive.indexed_keys(),
                archive.mapped_len(),
                archive.is_checksummed()
            );
        }
        "prefix-ids-mmap" => {
            let prefixes = required_path(args.next(), "prefix-ids-mmap requires a prefix path")?;
            let prefix = required_string(args.next(), "prefix-ids-mmap requires a prefix")?;
            let _access = preflight_search_archive_access(&prefixes, "prefix ids mmap")?;
            let archive = MmapPrefixArchive::open(prefixes)?;
            print_file_ids(archive.ids_for(&prefix)?);
        }
        "prefix-id-block-mmap" => {
            let prefixes =
                required_path(args.next(), "prefix-id-block-mmap requires a prefix path")?;
            let prefix = required_string(args.next(), "prefix-id-block-mmap requires a prefix")?;
            let block_index =
                parse_usize_arg(args.next(), "prefix-id-block-mmap requires a block index")?;
            let _access = preflight_search_archive_access(&prefixes, "prefix id block mmap")?;
            let archive = MmapPrefixArchive::open(prefixes)?;
            print_file_ids(archive.id_block_for(&prefix, block_index)?);
        }
        "prefix-verify" => {
            let prefixes = required_path(args.next(), "prefix-verify requires a prefix path")?;
            let _access = preflight_search_archive_access(&prefixes, "prefix verify")?;
            let archive = MmapPrefixArchive::open(prefixes)?;
            println!(
                "prefix-verify\tprefixes={}\tbytes={}\tchecksum={}",
                archive.indexed_prefixes(),
                archive.mapped_len(),
                archive.is_checksummed()
            );
        }
        "substring-ids-mmap" => {
            let substrings =
                required_path(args.next(), "substring-ids-mmap requires a substring path")?;
            let gram = required_string(args.next(), "substring-ids-mmap requires a trigram")?;
            let _access = preflight_search_archive_access(&substrings, "substring ids mmap")?;
            let archive = MmapSubstringArchive::open(substrings)?;
            print_file_ids(archive.ids_for(&gram)?);
        }
        "substring-id-block-mmap" => {
            let substrings = required_path(
                args.next(),
                "substring-id-block-mmap requires a substring path",
            )?;
            let gram = required_string(args.next(), "substring-id-block-mmap requires a trigram")?;
            let block_index = parse_usize_arg(
                args.next(),
                "substring-id-block-mmap requires a block index",
            )?;
            let _access = preflight_search_archive_access(&substrings, "substring id block mmap")?;
            let archive = MmapSubstringArchive::open(substrings)?;
            print_file_ids(archive.id_block_for(&gram, block_index)?);
        }
        "substring-verify" => {
            let substrings =
                required_path(args.next(), "substring-verify requires a substring path")?;
            let _access = preflight_search_archive_access(&substrings, "substring verify")?;
            let archive = MmapSubstringArchive::open(substrings)?;
            println!(
                "substring-verify\tgrams={}\tbytes={}\tchecksum={}",
                archive.indexed_grams(),
                archive.mapped_len(),
                archive.is_checksummed()
            );
        }
        "dictionary-lookup" => {
            let dictionary =
                required_path(args.next(), "dictionary-lookup requires a dictionary path")?;
            let term = required_string(args.next(), "dictionary-lookup requires a term")?;
            let _access = preflight_search_archive_access(&dictionary, "dictionary lookup")?;
            let archive = MmapDictionary::open(dictionary)?;
            match archive.find(&term)? {
                Some(index) => println!("dictionary\tfound\tindex={index}\tterm={term}"),
                None => println!("dictionary\tmissing\tterm={term}"),
            }
        }
        "dictionary-verify" => {
            let dictionary =
                required_path(args.next(), "dictionary-verify requires a dictionary path")?;
            let _access = preflight_search_archive_access(&dictionary, "dictionary verify")?;
            let archive = MmapDictionary::open(dictionary)?;
            println!(
                "dictionary-verify\tterms={}\tbytes={}\tchecksum={}",
                archive.len(),
                archive.mapped_len(),
                if archive.is_checksummed() {
                    "verified"
                } else {
                    "legacy"
                }
            );
        }
        "metadata-ids-mmap" => {
            let metadata =
                required_path(args.next(), "metadata-ids-mmap requires a metadata path")?;
            let field = parse_metadata_field(
                &required_string(args.next(), "metadata-ids-mmap requires a field")?,
                "metadata field",
            )?;
            let term = required_string(args.next(), "metadata-ids-mmap requires a term")?;
            let _access = preflight_search_archive_access(&metadata, "metadata ids mmap")?;
            let archive = MmapMetadataArchive::open(metadata)?;
            print_file_ids(archive.ids_for(field, &term)?);
        }
        "metadata-id-block-mmap" => {
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
            let _access = preflight_search_archive_access(&metadata, "metadata id block mmap")?;
            let archive = MmapMetadataArchive::open(metadata)?;
            print_file_ids(archive.id_block_for(field, &term, block_index)?);
        }
        "metadata-verify" => {
            let metadata = required_path(args.next(), "metadata-verify requires a metadata path")?;
            let _access = preflight_search_archive_access(&metadata, "metadata verify")?;
            let archive = MmapMetadataArchive::open(metadata)?;
            println!(
                "metadata-verify\tterms={}\tbytes={}\tchecksum={}",
                archive.indexed_terms(),
                archive.mapped_len(),
                if archive.is_checksummed() {
                    "verified"
                } else {
                    "legacy"
                }
            );
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn preflight_content_archive_access(path: &Path, worker: &str) -> Result<ScopedAccessGuard> {
    preflight_access_scope(path, AccessIntent::Read, worker)
}

fn preflight_search_archive_access(path: &Path, worker: &str) -> Result<ScopedAccessGuard> {
    preflight_access_scope(path, AccessIntent::Read, worker)
}

fn preflight_search_index_columns_access(
    records: &Path,
    columns: &Path,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(records, AccessIntent::Read, "search index columns records")?,
        preflight_access_scope(columns, AccessIntent::Read, "search index columns columns")?,
    ])
}

fn preflight_content_index_search_access(
    records: &Path,
    content: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(records, AccessIntent::Read, &format!("{worker} records"))?,
        preflight_access_scope(content, AccessIntent::Read, &format!("{worker} content"))?,
    ])
}

fn preflight_content_index_set_search_access(
    records: &Path,
    content_paths: &[PathBuf],
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = vec![preflight_access_scope(
        records,
        AccessIntent::Read,
        &format!("{worker} records"),
    )?];
    let content_worker = format!("{worker} content");
    guards.extend(preflight_content_archives_access(
        content_paths,
        &content_worker,
    )?);
    Ok(guards)
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

fn preflight_sidecar_index_search_access(
    paths: SidecarIndexAccessPaths<'_>,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        preflight_access_scope(
            paths.records,
            AccessIntent::Read,
            &format!("{worker} records"),
        )?,
        preflight_access_scope(
            paths.columns,
            AccessIntent::Read,
            &format!("{worker} columns"),
        )?,
        preflight_access_scope(
            paths.metadata,
            AccessIntent::Read,
            &format!("{worker} metadata"),
        )?,
        preflight_access_scope(
            paths.prefixes,
            AccessIntent::Read,
            &format!("{worker} prefixes"),
        )?,
        preflight_access_scope(
            paths.substrings,
            AccessIntent::Read,
            &format!("{worker} substrings"),
        )?,
        preflight_access_scope(paths.fuzzy, AccessIntent::Read, &format!("{worker} fuzzy"))?,
        preflight_access_scope(
            paths.content,
            AccessIntent::Read,
            &format!("{worker} content"),
        )?,
    ])
}

fn preflight_content_archives_access(
    paths: &[PathBuf],
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    paths
        .iter()
        .map(|path| preflight_content_archive_access(path, worker))
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

fn print_content_session_report(
    label: &str,
    content_archives: usize,
    report: &gfm_index::ContentQuerySessionReport,
) {
    eprintln!(
        "{label}\tcontent-archives={content_archives}\tcontent-keys={}\trecords-loaded={}\trecords-missing={}\tcandidate-ids={}\tfull-hydration={}\tposting-cache-hits={}\tposting-cache-misses={}\trecord-cache-hits={}\trecord-cache-misses={}",
        report.load.content_keys,
        report.load.records_loaded,
        report.load.records_missing,
        report.load.candidate_ids,
        report.load.full_hydration,
        report.posting_cache_hits,
        report.posting_cache_misses,
        report.record_cache_hits,
        report.record_cache_misses
    );
}

fn print_sidecar_session_report(
    label: &str,
    session: &SidecarIndexQuerySession,
    report: &SidecarQuerySessionReport,
    budget: SearchLookupBudget,
) {
    let hydration = &report.hydration;
    eprintln!(
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
    );
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
