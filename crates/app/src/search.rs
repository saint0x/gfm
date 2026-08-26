use crate::content::run_content_search;
use crate::extract::extraction_budget_profile;
use crate::{parse_required_scheduling_pressure, parse_usize_arg, required_path, required_string};
use gfm_content::Extractor;
use gfm_index::{
    query_sidecar_imports, Indexer, LiveIndex, SearchArchiveLookup, SearchLookupBudget,
    SearchRecordColumns, SearchStreamStage,
};
use gfm_store::{
    MmapContentArchive, MmapMetadataArchive, MmapRecordArchive, MmapRecordColumns,
    MmapSubstringArchive,
};
use gfm_types::{FileKind, Result, SearchHit};

pub(crate) fn run(command: &str, args: &mut impl Iterator<Item = String>) -> Result<bool> {
    match command {
        "search" => {
            let root = required_path(args.next(), "search requires a root path")?;
            let query = required_string(args.next(), "search requires a query string")?;
            let snapshot = Indexer::default().build(root)?;
            for hit in snapshot.search(&query, 50) {
                print_hit(&hit);
            }
        }
        "search-stream" => {
            let root = required_path(args.next(), "search-stream requires a root path")?;
            let query = required_string(args.next(), "search-stream requires a query string")?;
            let snapshot = Indexer::default().build(root)?;
            for batch in snapshot.stream_search(&query, 50)? {
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
        "search-index" => {
            let index_path = required_path(args.next(), "search-index requires an index path")?;
            let query = required_string(args.next(), "search-index requires a query string")?;
            let snapshot = Indexer::default().load(index_path)?;
            for hit in snapshot.search(&query, 50) {
                print_hit(&hit);
            }
        }
        "search-index-mmap" => {
            let index_path =
                required_path(args.next(), "search-index-mmap requires an index path")?;
            let query = required_string(args.next(), "search-index-mmap requires a query string")?;
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
            let records = MmapRecordArchive::open(records)?;
            let columns = MmapRecordColumns::open(columns)?;
            let metadata = MmapMetadataArchive::open(metadata)?;
            let substrings_archive = MmapSubstringArchive::open(&substrings)?;
            let lookup = SearchArchiveLookup::open(prefixes, substrings, fuzzy)?;
            let content = MmapContentArchive::open(content)?;
            let budget = SearchLookupBudget::default();
            let import = query_sidecar_imports(
                &metadata,
                &lookup,
                &substrings_archive,
                &content,
                &query,
                budget,
            )?;
            let (live, hydration) =
                LiveIndex::from_mmap_records_with_sidecar_import(&records, &columns, import)?;
            eprintln!(
                "columns-indexed {} records-loaded {} records-missing {} candidate-ids {} full-hydration {} metadata-keys {} prefix-keys {} substring-keys {} fuzzy-keys {} prefix-archive-keys {} substring-archive-keys {} fuzzy-archive-keys {} content-keys {} metadata-budget {} substring-budget {} content-budget {}",
                hydration.columns_applied,
                hydration.records_loaded,
                hydration.records_missing,
                hydration.import.candidate_ids,
                hydration.import.requires_full_record_hydration,
                hydration.metadata_keys,
                hydration.prefix_keys,
                hydration.substring_keys,
                hydration.fuzzy_keys,
                lookup.indexed_prefixes(),
                lookup.indexed_substring_grams(),
                lookup.indexed_fuzzy_keys(),
                hydration.content_keys,
                budget.max_metadata_ids_per_term,
                budget.max_substring_ids_per_gram,
                budget.max_content_ids_per_term
            );
            let report = live.search_with_lookup_budget(&query, 50, &lookup, budget)?;
            for hit in report.hits {
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
            let records = MmapRecordArchive::open(records)?;
            let columns = MmapRecordColumns::open(columns)?;
            let metadata = MmapMetadataArchive::open(metadata)?;
            let substrings_archive = MmapSubstringArchive::open(&substrings)?;
            let lookup = SearchArchiveLookup::open(prefixes, substrings, fuzzy)?;
            let content = MmapContentArchive::open(content)?;
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
            let import = query_sidecar_imports(
                &metadata,
                &lookup,
                &substrings_archive,
                &content,
                &query,
                budget,
            )?;
            let (live, hydration) =
                LiveIndex::from_mmap_records_with_sidecar_import(&records, &columns, import)?;
            let report = live.search_with_lookup_budget(&query, 50, &lookup, budget)?;
            eprintln!(
                "sidecar-budget\tcolumns-indexed={}\trecords-loaded={}\trecords-missing={}\tcandidate-ids={}\tfull-hydration={}\tmetadata-keys={}\tprefix-keys={}\tsubstring-keys={}\tfuzzy-keys={}\tcontent-keys={}\tmetadata-budget={max_content_ids}\tcontent-budget={max_content_ids}\tprefix-archive-keys={}\tsubstring-archive-keys={}\tfuzzy-archive-keys={}\tprefix-terms={}\tprefix-lookup-requests={}\tprefix-lookup-ids={}\tprefix-candidate-ids={}\tprefix-cache-hits={}\tprefix-cache-misses={}\tprefix-cutoff-terms={}\tprefix-truncated-terms={}\tsubstring-terms={}\tsubstring-grams={}\tsubstring-lookup-requests={}\tsubstring-lookup-ids={}\tsubstring-candidate-ids={}\tsubstring-cache-hits={}\tsubstring-cache-misses={}\tsubstring-cutoff-terms={}\tsubstring-term-truncated-grams={}\tsubstring-truncated-grams={}\tfuzzy-terms={}\tfuzzy-keys-read={}\tfuzzy-lookup-requests={}\tfuzzy-lookup-terms={}\tfuzzy-candidate-terms={}\tfuzzy-verified-candidates={}\tfuzzy-cache-hits={}\tfuzzy-cache-misses={}\tfuzzy-key-truncated-terms={}\tfuzzy-term-truncated-keys={}\tfuzzy-candidate-truncated-terms={}",
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
                lookup.indexed_prefixes(),
                lookup.indexed_substring_grams(),
                lookup.indexed_fuzzy_keys(),
                report.lookup.prefix_terms,
                report.lookup.prefix_lookup_requests,
                report.lookup.prefix_lookup_ids,
                report.lookup.prefix_candidate_ids,
                report.lookup.prefix_cache_hits,
                report.lookup.prefix_cache_misses,
                report.lookup.prefix_cutoff_terms,
                report.lookup.prefix_truncated_terms,
                report.lookup.substring_terms,
                report.lookup.substring_grams,
                report.lookup.substring_lookup_requests,
                report.lookup.substring_lookup_ids,
                report.lookup.substring_candidate_ids,
                report.lookup.substring_cache_hits,
                report.lookup.substring_cache_misses,
                report.lookup.substring_cutoff_terms,
                report.lookup.substring_term_truncated_grams,
                report.lookup.substring_truncated_grams,
                report.lookup.fuzzy_terms,
                report.lookup.fuzzy_keys,
                report.lookup.fuzzy_lookup_requests,
                report.lookup.fuzzy_lookup_terms,
                report.lookup.fuzzy_candidate_terms,
                report.lookup.fuzzy_verified_candidates,
                report.lookup.fuzzy_cache_hits,
                report.lookup.fuzzy_cache_misses,
                report.lookup.fuzzy_key_truncated_terms,
                report.lookup.fuzzy_term_truncated_keys,
                report.lookup.fuzzy_candidate_truncated_terms
            );
            for hit in report.hits {
                print_hit(&hit);
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn marker(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "dir",
        FileKind::File => "file",
        FileKind::Symlink => "link",
        FileKind::Other => "other",
    }
}

pub(crate) fn print_hit(hit: &SearchHit) {
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
