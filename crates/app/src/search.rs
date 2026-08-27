use crate::access::{preflight_access_scope, preflight_volume_access_scope, ScopedAccessGuard};
use crate::content::run_content_search;
use crate::extract::extraction_budget_profile;
use crate::runtime::run_volume_task_cancellable;
use crate::{
    detect_volume_id, parse_required_scheduling_pressure, parse_usize_arg, required_path,
    required_string,
};
use gfm_content::Extractor;
use gfm_index::{
    Indexer, LiveIndex, SearchLookupBudget, SearchRecordColumns, SearchStreamStage,
    SearchVolumeScope, SidecarIndexQuerySession, SidecarQuerySessionReport,
};
use gfm_jobs::Priority;
use gfm_mac::AccessIntent;
use gfm_store::{
    ContentArchive, ContentArchiveManifest, MetadataField, MmapContentArchive, MmapContentSet,
    MmapDictionary, MmapFuzzyArchive, MmapMetadataArchive, MmapPrefixArchive, MmapRecordArchive,
    MmapRecordColumns, MmapSubstringArchive,
};
use gfm_types::{FileKind, GfmError, Result, SearchHit, VolumeId};
use std::path::{Path, PathBuf};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "search" => {
            let root = required_path(args.next(), "search requires a root path")?;
            let query = required_string(args.next(), "search requires a query string")?;
            preflight_volume_access_scope(&root, AccessIntent::Index, "search")?;
            let volume = detect_volume_id(&root).ok();
            let hits = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "search",
                move |cancellation| {
                    let _access = preflight_access_scope(&root, AccessIntent::Index, "search")?;
                    let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
                    let session = snapshot.query_session();
                    Ok(session.search(&query, 50))
                },
            )?;
            for hit in hits {
                print_hit(&hit);
            }
        }
        "search-stream" => {
            let root = required_path(args.next(), "search-stream requires a root path")?;
            let query = required_string(args.next(), "search-stream requires a query string")?;
            preflight_volume_access_scope(&root, AccessIntent::Index, "search stream")?;
            let volume = detect_volume_id(&root).ok();
            let batches = run_volume_task_cancellable(
                volume,
                Priority::Visible,
                "search stream",
                move |cancellation| {
                    let _access =
                        preflight_access_scope(&root, AccessIntent::Index, "search stream")?;
                    let snapshot = Indexer::default().build_cancellable(root, &cancellation)?;
                    let session = snapshot.query_session();
                    session.stream_search(&query, 50)
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
        "search-content-index" => {
            let records =
                required_path(args.next(), "search-content-index requires a records path")?;
            let content =
                required_path(args.next(), "search-content-index requires a content path")?;
            let query =
                required_string(args.next(), "search-content-index requires a query string")?;
            let output = run_content_index_search(
                records,
                content,
                query,
                Extractor::default(),
                "content index search",
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
            )?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
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
            let output = run_content_index_set_search(records, content_paths, query)?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
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
            let output = run_content_index_set_session(records, content_paths, query)?;
            for diagnostic in output.diagnostics {
                eprintln!("{diagnostic}");
            }
            for hit in output.hits {
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
            let output = run_content_index_manifest_search(records, manifest, query)?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
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
            let output = run_content_index_manifest_session(records, manifest, query)?;
            for diagnostic in output.diagnostics {
                eprintln!("{diagnostic}");
            }
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "search-index" => {
            let index_path = required_path(args.next(), "search-index requires an index path")?;
            let query = required_string(args.next(), "search-index requires a query string")?;
            let hits = run_search_archive_read(index_path, "search index", move |index_path| {
                let session = Indexer::default().load_query_session(index_path)?;
                Ok(session.search(&query, 50))
            })?;
            for hit in hits {
                print_hit(&hit);
            }
        }
        "search-index-mmap" => {
            let index_path =
                required_path(args.next(), "search-index-mmap requires an index path")?;
            let query = required_string(args.next(), "search-index-mmap requires a query string")?;
            let hits =
                run_search_archive_read(index_path, "search index mmap", move |index_path| {
                    let live =
                        LiveIndex::from_records(MmapRecordArchive::open(index_path)?.records()?);
                    Ok(live.search(&query, 50))
                })?;
            for hit in hits {
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
            let output = run_search_index_columns(records, columns, query)?;
            eprintln!("columns-indexed {}", output.columns_applied);
            for hit in output.hits {
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
            let sidecars = OwnedSidecarIndexAccessPaths {
                records,
                columns,
                metadata,
                prefixes,
                substrings,
                fuzzy,
                content,
            };
            let output = run_sidecar_index_search(sidecars, query)?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
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
            let sidecars = OwnedSidecarIndexAccessPaths {
                records,
                columns,
                metadata,
                prefixes,
                substrings,
                fuzzy,
                content,
            };
            let output = run_sidecar_index_session(sidecars, query)?;
            for diagnostic in output.diagnostics {
                eprintln!("{diagnostic}");
            }
            for hit in output.hits {
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
            let output = run_sidecar_index_budget(sidecars, query, budget)?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
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
            let output = run_sidecar_index_volume_scope(sidecars, query, scope, budget)?;
            eprintln!("{}", output.diagnostics);
            for hit in output.hits {
                print_hit(&hit);
            }
        }
        "content-ids" => {
            let content = required_path(args.next(), "content-ids requires a content path")?;
            let term = required_string(args.next(), "content-ids requires a term")?;
            let ids = run_content_archive_read(content, "content ids", move |content| {
                let mut archive = ContentArchive::open(content)?;
                archive.ids_for_term(&term)
            })?;
            print_file_ids(ids);
        }
        "content-ids-mmap" => {
            let content = required_path(args.next(), "content-ids-mmap requires a content path")?;
            let term = required_string(args.next(), "content-ids-mmap requires a term")?;
            let ids = run_content_archive_read(content, "content ids mmap", move |content| {
                let archive = MmapContentArchive::open(content)?;
                archive.ids_for_term(&term)
            })?;
            print_file_ids(ids);
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
            let _access =
                preflight_content_manifest_access(&manifest, "content ids mmap manifest")?;
            let archive = MmapContentSet::open_manifest(manifest)?;
            print_file_ids(archive.ids_for_term(&term)?);
        }
        "content-id-block-mmap" => {
            let content =
                required_path(args.next(), "content-id-block-mmap requires a content path")?;
            let term = required_string(args.next(), "content-id-block-mmap requires a term")?;
            let block_index =
                parse_usize_arg(args.next(), "content-id-block-mmap requires a block index")?;
            let ids = run_content_archive_read(content, "content id block mmap", move |content| {
                let archive = MmapContentArchive::open(content)?;
                archive.id_block_for_term(&term, block_index)
            })?;
            print_file_ids(ids);
        }
        "content-verify" => {
            let content = required_path(args.next(), "content-verify requires a content path")?;
            let report = run_content_archive_read(content, "content verify", move |content| {
                let archive = MmapContentArchive::open(content)?;
                Ok(format!(
                    "content-verify\tterms={}\tbytes={}\tchecksum={}",
                    archive.indexed_terms(),
                    archive.mapped_len(),
                    if archive.is_checksummed() {
                        "verified"
                    } else {
                        "legacy"
                    }
                ))
            })?;
            println!("{report}");
        }
        "fuzzy-terms-mmap" => {
            let fuzzy = required_path(args.next(), "fuzzy-terms-mmap requires a fuzzy path")?;
            let key = required_string(args.next(), "fuzzy-terms-mmap requires a key")?;
            let terms = run_search_archive_read(fuzzy, "fuzzy terms mmap", move |fuzzy| {
                let archive = MmapFuzzyArchive::open(fuzzy)?;
                archive.terms_for(&key)
            })?;
            for term in terms {
                println!("{term}");
            }
        }
        "fuzzy-verify" => {
            let fuzzy = required_path(args.next(), "fuzzy-verify requires a fuzzy path")?;
            let report = run_search_archive_read(fuzzy, "fuzzy verify", move |fuzzy| {
                let archive = MmapFuzzyArchive::open(fuzzy)?;
                Ok(format!(
                    "fuzzy-verify\tkeys={}\tbytes={}\tchecksum={}",
                    archive.indexed_keys(),
                    archive.mapped_len(),
                    archive.is_checksummed()
                ))
            })?;
            println!("{report}");
        }
        "prefix-ids-mmap" => {
            let prefixes = required_path(args.next(), "prefix-ids-mmap requires a prefix path")?;
            let prefix = required_string(args.next(), "prefix-ids-mmap requires a prefix")?;
            let ids = run_search_archive_read(prefixes, "prefix ids mmap", move |prefixes| {
                let archive = MmapPrefixArchive::open(prefixes)?;
                archive.ids_for(&prefix)
            })?;
            print_file_ids(ids);
        }
        "prefix-id-block-mmap" => {
            let prefixes =
                required_path(args.next(), "prefix-id-block-mmap requires a prefix path")?;
            let prefix = required_string(args.next(), "prefix-id-block-mmap requires a prefix")?;
            let block_index =
                parse_usize_arg(args.next(), "prefix-id-block-mmap requires a block index")?;
            let ids = run_search_archive_read(prefixes, "prefix id block mmap", move |prefixes| {
                let archive = MmapPrefixArchive::open(prefixes)?;
                archive.id_block_for(&prefix, block_index)
            })?;
            print_file_ids(ids);
        }
        "prefix-verify" => {
            let prefixes = required_path(args.next(), "prefix-verify requires a prefix path")?;
            let report = run_search_archive_read(prefixes, "prefix verify", move |prefixes| {
                let archive = MmapPrefixArchive::open(prefixes)?;
                Ok(format!(
                    "prefix-verify\tprefixes={}\tbytes={}\tchecksum={}",
                    archive.indexed_prefixes(),
                    archive.mapped_len(),
                    archive.is_checksummed()
                ))
            })?;
            println!("{report}");
        }
        "substring-ids-mmap" => {
            let substrings =
                required_path(args.next(), "substring-ids-mmap requires a substring path")?;
            let gram = required_string(args.next(), "substring-ids-mmap requires a trigram")?;
            let ids =
                run_search_archive_read(substrings, "substring ids mmap", move |substrings| {
                    let archive = MmapSubstringArchive::open(substrings)?;
                    archive.ids_for(&gram)
                })?;
            print_file_ids(ids);
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
            let ids = run_search_archive_read(
                substrings,
                "substring id block mmap",
                move |substrings| {
                    let archive = MmapSubstringArchive::open(substrings)?;
                    archive.id_block_for(&gram, block_index)
                },
            )?;
            print_file_ids(ids);
        }
        "substring-verify" => {
            let substrings =
                required_path(args.next(), "substring-verify requires a substring path")?;
            let report =
                run_search_archive_read(substrings, "substring verify", move |substrings| {
                    let archive = MmapSubstringArchive::open(substrings)?;
                    Ok(format!(
                        "substring-verify\tgrams={}\tbytes={}\tchecksum={}",
                        archive.indexed_grams(),
                        archive.mapped_len(),
                        archive.is_checksummed()
                    ))
                })?;
            println!("{report}");
        }
        "dictionary-lookup" => {
            let dictionary =
                required_path(args.next(), "dictionary-lookup requires a dictionary path")?;
            let term = required_string(args.next(), "dictionary-lookup requires a term")?;
            let report =
                run_search_archive_read(dictionary, "dictionary lookup", move |dictionary| {
                    let archive = MmapDictionary::open(dictionary)?;
                    Ok(match archive.find(&term)? {
                        Some(index) => format!("dictionary\tfound\tindex={index}\tterm={term}"),
                        None => format!("dictionary\tmissing\tterm={term}"),
                    })
                })?;
            println!("{report}");
        }
        "dictionary-verify" => {
            let dictionary =
                required_path(args.next(), "dictionary-verify requires a dictionary path")?;
            let report =
                run_search_archive_read(dictionary, "dictionary verify", move |dictionary| {
                    let archive = MmapDictionary::open(dictionary)?;
                    Ok(format!(
                        "dictionary-verify\tterms={}\tbytes={}\tchecksum={}",
                        archive.len(),
                        archive.mapped_len(),
                        if archive.is_checksummed() {
                            "verified"
                        } else {
                            "legacy"
                        }
                    ))
                })?;
            println!("{report}");
        }
        "metadata-ids-mmap" => {
            let metadata =
                required_path(args.next(), "metadata-ids-mmap requires a metadata path")?;
            let field = parse_metadata_field(
                &required_string(args.next(), "metadata-ids-mmap requires a field")?,
                "metadata field",
            )?;
            let term = required_string(args.next(), "metadata-ids-mmap requires a term")?;
            let ids = run_search_archive_read(metadata, "metadata ids mmap", move |metadata| {
                let archive = MmapMetadataArchive::open(metadata)?;
                archive.ids_for(field, &term)
            })?;
            print_file_ids(ids);
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
            let ids =
                run_search_archive_read(metadata, "metadata id block mmap", move |metadata| {
                    let archive = MmapMetadataArchive::open(metadata)?;
                    archive.id_block_for(field, &term, block_index)
                })?;
            print_file_ids(ids);
        }
        "metadata-verify" => {
            let metadata = required_path(args.next(), "metadata-verify requires a metadata path")?;
            let report = run_search_archive_read(metadata, "metadata verify", move |metadata| {
                let archive = MmapMetadataArchive::open(metadata)?;
                Ok(format!(
                    "metadata-verify\tterms={}\tbytes={}\tchecksum={}",
                    archive.indexed_terms(),
                    archive.mapped_len(),
                    if archive.is_checksummed() {
                        "verified"
                    } else {
                        "legacy"
                    }
                ))
            })?;
            println!("{report}");
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn preflight_content_archive_access(path: &Path, worker: &str) -> Result<ScopedAccessGuard> {
    preflight_access_scope(path, AccessIntent::Read, worker)
}

fn run_content_archive_read<T>(
    path: PathBuf,
    worker: &'static str,
    read: impl FnOnce(PathBuf) -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    preflight_volume_access_scope(&path, AccessIntent::Read, worker)?;
    let volume = detect_volume_id(&path).ok();
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_content_archive_access(&path, worker)?;
        cancellation.check()?;
        read(path)
    })
}

fn run_search_archive_read<T>(
    path: PathBuf,
    worker: &'static str,
    read: impl FnOnce(PathBuf) -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    preflight_volume_access_scope(&path, AccessIntent::Read, worker)?;
    let volume = detect_volume_id(&path).ok();
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_search_archive_access(&path, worker)?;
        cancellation.check()?;
        read(path)
    })
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
) -> Result<SearchIndexColumnsOutput> {
    preflight_volume_access_scope(&records, AccessIntent::Read, "search index columns records")?;
    preflight_volume_access_scope(&columns, AccessIntent::Read, "search index columns columns")?;
    let volume = detect_volume_id(&records)
        .ok()
        .or_else(|| detect_volume_id(&columns).ok());
    run_volume_task_cancellable(
        volume,
        Priority::Visible,
        "search index columns",
        move |cancellation| {
            cancellation.check()?;
            let _access = preflight_search_index_columns_access(&records, &columns)?;
            cancellation.check()?;
            let records = MmapRecordArchive::open(records)?;
            let columns = MmapRecordColumns::open(columns)?;
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
            let (live, columns_applied) =
                LiveIndex::from_records_with_columns(records.records()?, search_columns);
            cancellation.check()?;
            Ok(SearchIndexColumnsOutput {
                columns_applied,
                hits: live.search(&query, 50),
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
) -> Result<ContentIndexSearchOutput> {
    preflight_volume_access_scope(&records, AccessIntent::Read, &format!("{worker} records"))?;
    preflight_volume_access_scope(&content, AccessIntent::Read, &format!("{worker} content"))?;
    let volume = detect_volume_id(&records)
        .ok()
        .or_else(|| detect_volume_id(&content).ok());
    run_volume_task_cancellable(volume, Priority::Visible, worker, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_content_index_search_access(&records, &content, worker)?;
        cancellation.check()?;
        let (live, report) =
            Indexer::default().load_live_with_content_for_query(records, content, &query)?;
        let diagnostics = format!(
            "content-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
            report.content_keys,
            report.records_loaded,
            report.records_missing,
            report.candidate_ids,
            report.full_hydration
        );
        cancellation.check()?;
        let hits = live.search_with_snippets(&query, 50, &extractor, 96)?;
        Ok(ContentIndexSearchOutput { diagnostics, hits })
    })
}

fn run_content_index_set_search(
    records: PathBuf,
    content_paths: Vec<PathBuf>,
    query: String,
) -> Result<ContentIndexSetSearchOutput> {
    const WORKER: &str = "content index set search";
    preflight_content_index_set_volume_access(&records, &content_paths, WORKER)?;
    let volume = detect_volume_id(&records).ok();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_content_index_set_search_access(&records, &content_paths, WORKER)?;
        cancellation.check()?;
        let archive_count = content_paths.len();
        let (live, report) =
            Indexer::default().load_live_with_content_set(records, &content_paths, &query)?;
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
        Ok(ContentIndexSetSearchOutput {
            diagnostics,
            hits: live.search(&query, 50),
        })
    })
}

fn run_content_index_set_session(
    records: PathBuf,
    content_paths: Vec<PathBuf>,
    query: String,
) -> Result<ContentIndexSessionOutput> {
    const WORKER: &str = "content index set session";
    preflight_content_index_set_volume_access(&records, &content_paths, WORKER)?;
    let volume = detect_volume_id(&records).ok();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_content_index_set_search_access(&records, &content_paths, WORKER)?;
        cancellation.check()?;
        let session =
            Indexer::default().load_content_set_query_session(&records, &content_paths)?;
        let first = session.search(&query, 50)?;
        let mut diagnostics = vec![format_content_session_report(
            "content-session-first",
            content_paths.len(),
            &first,
        )];
        let mut hits = first.search.hits;
        cancellation.check()?;
        let second = session.search(&query, 50)?;
        diagnostics.push(format_content_session_report(
            "content-session-second",
            content_paths.len(),
            &second,
        ));
        hits.extend(second.search.hits);
        Ok(ContentIndexSessionOutput { diagnostics, hits })
    })
}

fn run_content_index_manifest_search(
    records: PathBuf,
    manifest: PathBuf,
    query: String,
) -> Result<ContentIndexSetSearchOutput> {
    const WORKER: &str = "content index manifest search";
    preflight_volume_access_scope(&records, AccessIntent::Read, &format!("{WORKER} records"))?;
    preflight_volume_access_scope(&manifest, AccessIntent::Read, &format!("{WORKER} manifest"))?;
    let volume = detect_volume_id(&records)
        .ok()
        .or_else(|| detect_volume_id(&manifest).ok());
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_content_index_manifest_search_access(&records, &manifest, WORKER)?;
        cancellation.check()?;
        let (live, report) =
            Indexer::default().load_live_with_content_manifest(records, manifest, &query)?;
        let diagnostics = format!(
            "content-manifest-keys {} records-loaded {} records-missing {} candidate-ids {} full-hydration {}",
            report.content_keys,
            report.records_loaded,
            report.records_missing,
            report.candidate_ids,
            report.full_hydration
        );
        cancellation.check()?;
        Ok(ContentIndexSetSearchOutput {
            diagnostics,
            hits: live.search(&query, 50),
        })
    })
}

fn run_content_index_manifest_session(
    records: PathBuf,
    manifest: PathBuf,
    query: String,
) -> Result<ContentIndexSessionOutput> {
    const WORKER: &str = "content index manifest session";
    preflight_volume_access_scope(&records, AccessIntent::Read, &format!("{WORKER} records"))?;
    preflight_volume_access_scope(&manifest, AccessIntent::Read, &format!("{WORKER} manifest"))?;
    let volume = detect_volume_id(&records)
        .ok()
        .or_else(|| detect_volume_id(&manifest).ok());
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_content_index_manifest_search_access(&records, &manifest, WORKER)?;
        cancellation.check()?;
        let session =
            Indexer::default().load_content_manifest_query_session(&records, &manifest)?;
        let archive_count = session.archive_count();
        let first = session.search(&query, 50)?;
        let mut diagnostics = vec![format_content_session_report(
            "content-manifest-session-first",
            archive_count,
            &first,
        )];
        let mut hits = first.search.hits;
        cancellation.check()?;
        let second = session.search(&query, 50)?;
        diagnostics.push(format_content_session_report(
            "content-manifest-session-second",
            archive_count,
            &second,
        ));
        hits.extend(second.search.hits);
        Ok(ContentIndexSessionOutput { diagnostics, hits })
    })
}

fn preflight_content_index_set_volume_access(
    records: &Path,
    content_paths: &[PathBuf],
    worker: &str,
) -> Result<()> {
    preflight_volume_access_scope(records, AccessIntent::Read, &format!("{worker} records"))?;
    for content in content_paths {
        preflight_volume_access_scope(content, AccessIntent::Read, &format!("{worker} content"))?;
    }
    Ok(())
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

fn preflight_content_index_manifest_search_access(
    records: &Path,
    manifest_path: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = vec![
        preflight_access_scope(records, AccessIntent::Read, &format!("{worker} records"))?,
        preflight_access_scope(
            manifest_path,
            AccessIntent::Read,
            &format!("{worker} manifest"),
        )?,
    ];
    let manifest = ContentArchiveManifest::read(manifest_path)?;
    let content_worker = format!("{worker} content");
    guards.extend(preflight_content_archives_access(
        &manifest.resolved_archive_paths(manifest_path),
        &content_worker,
    )?);
    Ok(guards)
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

    fn paths_with_roles(&self) -> [(&Path, &'static str); 7] {
        [
            (&self.records, "records"),
            (&self.columns, "columns"),
            (&self.metadata, "metadata"),
            (&self.prefixes, "prefixes"),
            (&self.substrings, "substrings"),
            (&self.fuzzy, "fuzzy"),
            (&self.content, "content"),
        ]
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
) -> Result<SidecarSearchOutput> {
    const WORKER: &str = "sidecar search";
    preflight_sidecar_index_volume_access(&paths, WORKER)?;
    let volume = detect_volume_id(&paths.records).ok();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_sidecar_index_search_access(paths.borrowed(), WORKER)?;
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
        let session = SidecarIndexQuerySession::open(
            records, columns, metadata, prefixes, substrings, fuzzy, content,
        )?;
        cancellation.check()?;
        let budget = SearchLookupBudget::default();
        let report = session.search_with_budget(&query, 50, budget)?;
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
    })
}

fn run_sidecar_index_session(
    paths: OwnedSidecarIndexAccessPaths,
    query: String,
) -> Result<SidecarSessionOutput> {
    const WORKER: &str = "sidecar session";
    preflight_sidecar_index_volume_access(&paths, WORKER)?;
    let volume = detect_volume_id(&paths.records).ok();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_sidecar_index_search_access(paths.borrowed(), WORKER)?;
        cancellation.check()?;
        let session = open_sidecar_index_query_session(paths)?;
        cancellation.check()?;
        let budget = SearchLookupBudget::default();
        let first = session.search_with_budget(&query, 50, budget)?;
        cancellation.check()?;
        let second = session.search_with_budget(&query, 50, budget)?;
        Ok(SidecarSessionOutput {
            diagnostics: vec![
                format_sidecar_session_report("sidecar-session-first", &session, &first, budget),
                format_sidecar_session_report("sidecar-session-second", &session, &second, budget),
            ],
            hits: second.search.hits,
        })
    })
}

fn run_sidecar_index_budget(
    paths: OwnedSidecarIndexAccessPaths,
    query: String,
    budget: SearchLookupBudget,
) -> Result<SidecarSearchOutput> {
    const WORKER: &str = "sidecar budget";
    preflight_sidecar_index_volume_access(&paths, WORKER)?;
    let volume = detect_volume_id(&paths.records).ok();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_sidecar_index_search_access(paths.borrowed(), WORKER)?;
        cancellation.check()?;
        let session = open_sidecar_index_query_session(paths)?;
        cancellation.check()?;
        let report = session.search_with_budget(&query, 50, budget)?;
        Ok(SidecarSearchOutput {
            diagnostics: format_sidecar_budget_report(&session, &report, budget),
            hits: report.search.hits,
        })
    })
}

fn run_sidecar_index_volume_scope(
    paths: OwnedSidecarIndexAccessPaths,
    query: String,
    scope: SearchVolumeScope,
    budget: SearchLookupBudget,
) -> Result<SidecarSearchOutput> {
    const WORKER: &str = "sidecar volume scope";
    preflight_sidecar_index_volume_access(&paths, WORKER)?;
    let volume = detect_volume_id(&paths.records).ok();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = preflight_sidecar_index_search_access(paths.borrowed(), WORKER)?;
        cancellation.check()?;
        let session = open_sidecar_index_query_session(paths)?;
        cancellation.check()?;
        let report = session.search_with_volume_scope(&query, 50, &scope)?;
        Ok(SidecarSearchOutput {
            diagnostics: format_sidecar_session_report(
                "sidecar-volume-scope",
                &session,
                &report,
                budget,
            ),
            hits: report.search.hits,
        })
    })
}

fn open_sidecar_index_query_session(
    paths: OwnedSidecarIndexAccessPaths,
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
    SidecarIndexQuerySession::open(
        records, columns, metadata, prefixes, substrings, fuzzy, content,
    )
}

fn preflight_sidecar_index_volume_access(
    paths: &OwnedSidecarIndexAccessPaths,
    worker: &str,
) -> Result<()> {
    for (path, role) in paths.paths_with_roles() {
        preflight_volume_access_scope(path, AccessIntent::Read, &format!("{worker} {role}"))?;
    }
    Ok(())
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

fn preflight_content_manifest_access(
    manifest_path: &Path,
    worker: &str,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = vec![preflight_content_archive_access(manifest_path, worker)?];
    let manifest = ContentArchiveManifest::read(manifest_path)?;
    guards.extend(preflight_content_archives_access(
        &manifest.resolved_archive_paths(manifest_path),
        worker,
    )?);
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
