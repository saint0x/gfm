use super::columns::RecordColumns;
use super::fuzzy::deletion_keys;
use super::query::{normalize, tokenize};
use super::terms::{
    is_fuzzy_term, is_prefix_term, is_substring_gram, path_key, substring_grams, token_prefixes,
};
use super::{
    SearchFuzzyPosting, SearchIndex, SearchMetadataField, SearchMetadataPosting,
    SearchPrefixPosting, SearchRecordColumns, SearchSubstringPosting,
};
use gfm_types::{ContentPositions, ContentPosting, FileId, FileRecord};
use std::collections::{BTreeMap, BTreeSet};

impl SearchIndex {
    pub fn indexed_name_prefixes(&self) -> usize {
        self.name_prefixes.len()
    }

    pub fn insert(&mut self, record: FileRecord) {
        let id = record.id;
        if let Some(old) = self.records.remove(&id) {
            self.remove_terms(&old);
            self.columns.remove(&id);
            self.paths.remove(&path_key(&old.path));
        }
        let columns = RecordColumns::from_record(&record);
        self.add_terms(&record, &columns);
        self.paths.insert(path_key(&record.path), id);
        self.columns.insert(id, columns);
        self.records.insert(id, record);
    }

    pub fn insert_with_columns(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
    ) -> bool {
        self.insert_with_columns_inner(record, columns, true, true, true, true)
    }

    pub fn insert_with_columns_deferred_fuzzy(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
    ) -> bool {
        self.insert_with_columns_inner(record, columns, true, true, false, true)
    }

    pub fn insert_with_columns_deferred_sidecars(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
    ) -> bool {
        self.insert_with_columns_inner(record, columns, false, false, false, false)
    }

    fn insert_with_columns_inner(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
        build_prefixes: bool,
        build_substrings: bool,
        build_fuzzy: bool,
        build_metadata: bool,
    ) -> bool {
        if record.id != columns.id {
            self.insert(record);
            return false;
        }
        let id = record.id;
        if let Some(old) = self.records.remove(&id) {
            self.remove_terms(&old);
            self.columns.remove(&id);
            self.paths.remove(&path_key(&old.path));
        }
        let normalized = RecordColumns::from_search_columns(&columns);
        self.add_terms_with_sidecar_policy(
            &record,
            &normalized,
            build_prefixes,
            build_substrings,
            build_fuzzy,
            build_metadata,
        );
        self.paths.insert(path_key(&record.path), id);
        self.columns.insert(id, normalized);
        self.records.insert(id, record);
        true
    }

    pub fn import_prefix_postings(&mut self, postings: &[SearchPrefixPosting]) -> usize {
        for posting in postings {
            let prefix = normalize(&posting.prefix);
            if !is_prefix_term(&prefix) {
                continue;
            }
            let ids = posting
                .ids
                .iter()
                .copied()
                .filter(|id| self.records.contains_key(id))
                .collect::<BTreeSet<_>>();
            if !ids.is_empty() {
                self.name_prefixes.entry(prefix).or_default().extend(ids);
            }
        }
        self.name_prefixes.len()
    }

    pub fn import_substring_postings(&mut self, postings: &[SearchSubstringPosting]) -> usize {
        for posting in postings {
            let gram = normalize(&posting.gram);
            if !is_substring_gram(&gram) {
                continue;
            }
            let ids = posting
                .ids
                .iter()
                .copied()
                .filter(|id| self.records.contains_key(id))
                .collect::<BTreeSet<_>>();
            if !ids.is_empty() {
                self.name_substrings.entry(gram).or_default().extend(ids);
            }
        }
        self.name_substrings.len()
    }

    pub fn import_fuzzy_postings(&mut self, postings: &[SearchFuzzyPosting]) -> usize {
        for posting in postings {
            let key = normalize(&posting.key);
            if key.is_empty() {
                continue;
            }
            let terms = posting
                .terms
                .iter()
                .map(|term| normalize(term))
                .filter(|term| is_fuzzy_term(term))
                .collect::<BTreeSet<_>>();
            if !terms.is_empty() {
                self.fuzzy_terms.entry(key).or_default().extend(terms);
            }
        }
        self.fuzzy_terms.len()
    }

    pub fn import_metadata_postings(&mut self, postings: &[SearchMetadataPosting]) -> usize {
        for posting in postings {
            let term = normalize(&posting.term);
            if term.is_empty() {
                continue;
            }
            let ids = posting
                .ids
                .iter()
                .copied()
                .filter(|id| self.records.contains_key(id))
                .collect::<BTreeSet<_>>();
            if ids.is_empty() {
                continue;
            }
            match posting.field {
                SearchMetadataField::Tag => self.tags.entry(term).or_default().extend(ids),
                SearchMetadataField::Comment => {
                    self.metadata_terms.entry(term).or_default().extend(ids)
                }
            }
        }
        self.tags.len() + self.metadata_terms.len()
    }

    pub fn apply_record_columns(&mut self, columns: SearchRecordColumns) -> bool {
        let Some(record) = self.records.get(&columns.id).cloned() else {
            return false;
        };
        self.remove_terms(&record);
        let normalized = RecordColumns::from_search_columns(&columns);
        self.add_terms(&record, &normalized);
        self.columns.insert(columns.id, normalized);
        true
    }

    pub fn remove(&mut self, id: FileId) -> Option<FileRecord> {
        let record = self.records.remove(&id)?;
        self.remove_terms(&record);
        self.columns.remove(&id);
        self.pinned.remove(&id);
        self.paths.remove(&path_key(&record.path));
        Some(record)
    }

    pub fn remove_path(&mut self, path: impl AsRef<std::path::Path>) -> Option<FileRecord> {
        let id = self.paths.remove(&path_key(path.as_ref()))?;
        self.remove(id)
    }

    pub fn remove_subtree(&mut self, root: impl AsRef<std::path::Path>) -> Vec<FileRecord> {
        let root = path_key(root.as_ref());
        let prefix = format!("{root}/");
        let ids: Vec<_> = self
            .paths
            .iter()
            .filter_map(|(path, id)| (path == &root || path.starts_with(&prefix)).then_some(*id))
            .collect();

        let mut removed = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(record) = self.remove(id) {
                removed.push(record);
            }
        }
        removed
    }

    pub fn get_path(&self, path: impl AsRef<std::path::Path>) -> Option<&FileRecord> {
        let id = self.paths.get(&path_key(path.as_ref()))?;
        self.records.get(id)
    }

    pub fn records(&self) -> impl Iterator<Item = &FileRecord> {
        self.records.values()
    }

    pub fn pin(&mut self, id: FileId) -> bool {
        if self.records.contains_key(&id) {
            self.pinned.insert(id)
        } else {
            false
        }
    }

    pub fn unpin(&mut self, id: FileId) -> bool {
        self.pinned.remove(&id)
    }

    pub fn is_pinned(&self, id: FileId) -> bool {
        self.pinned.contains(&id)
    }

    #[cfg(test)]
    pub(crate) fn name_prefix_posting_count(&self, prefix: &str) -> usize {
        self.name_prefixes
            .get(prefix)
            .map(BTreeSet::len)
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn content_record_term_count(&self, id: FileId) -> usize {
        self.content_record_terms
            .get(&id)
            .map(BTreeSet::len)
            .unwrap_or(0)
    }

    pub fn insert_content(&mut self, id: FileId, text: &str) {
        if !self.records.contains_key(&id) {
            return;
        }
        self.remove_content(id);
        for (position, token) in tokenize(&normalize(text)).into_iter().enumerate() {
            self.content_record_terms
                .entry(id)
                .or_default()
                .insert(token.clone());
            self.content_terms
                .entry(token)
                .or_default()
                .entry(id)
                .or_default()
                .push(position as u32);
        }
    }

    pub fn insert_content_terms(&mut self, id: FileId, terms: impl IntoIterator<Item = String>) {
        if !self.records.contains_key(&id) {
            return;
        }
        for term in terms {
            let term = normalize(&term);
            if !term.is_empty() {
                self.content_record_terms
                    .entry(id)
                    .or_default()
                    .insert(term.clone());
                self.content_terms
                    .entry(term)
                    .or_default()
                    .entry(id)
                    .or_default();
            }
        }
    }

    pub fn import_content_postings(&mut self, postings: &[ContentPosting]) {
        for posting in postings {
            let term = normalize(&posting.term);
            if term.is_empty() {
                continue;
            }
            for id in &posting.ids {
                if self.records.contains_key(id) {
                    self.content_record_terms
                        .entry(*id)
                        .or_default()
                        .insert(term.clone());
                    self.content_terms
                        .entry(term.clone())
                        .or_default()
                        .entry(*id)
                        .or_default();
                }
            }
            for positions in &posting.positions {
                if self.records.contains_key(&positions.id) {
                    let mut normalized_positions = positions.positions.clone();
                    normalized_positions.sort_unstable();
                    normalized_positions.dedup();
                    self.content_record_terms
                        .entry(positions.id)
                        .or_default()
                        .insert(term.clone());
                    self.content_terms
                        .entry(term.clone())
                        .or_default()
                        .insert(positions.id, normalized_positions);
                }
            }
        }
    }

    pub fn remove_content(&mut self, id: FileId) {
        let Some(terms) = self.content_record_terms.remove(&id) else {
            for positions in self.content_terms.values_mut() {
                positions.remove(&id);
            }
            self.content_terms
                .retain(|_, positions| !positions.is_empty());
            return;
        };
        for term in terms {
            if let Some(positions) = self.content_terms.get_mut(&term) {
                positions.remove(&id);
                if positions.is_empty() {
                    self.content_terms.remove(&term);
                }
            }
        }
    }

    pub fn content_postings(&self) -> Vec<ContentPosting> {
        self.content_terms
            .iter()
            .map(|(term, positions)| ContentPosting {
                term: term.clone(),
                ids: positions.keys().copied().collect(),
                positions: positions
                    .iter()
                    .filter(|(_, positions)| !positions.is_empty())
                    .map(|(id, positions)| ContentPositions {
                        id: *id,
                        positions: positions.clone(),
                    })
                    .collect(),
            })
            .collect()
    }

    fn add_terms(&mut self, record: &FileRecord, columns: &RecordColumns) {
        self.add_terms_with_sidecar_policy(record, columns, true, true, true, true);
    }

    fn add_terms_with_sidecar_policy(
        &mut self,
        record: &FileRecord,
        columns: &RecordColumns,
        build_prefixes: bool,
        build_substrings: bool,
        build_fuzzy: bool,
        build_metadata: bool,
    ) {
        self.name_exact
            .entry(columns.name.clone())
            .or_default()
            .insert(record.id);
        for token in &columns.name_tokens {
            let is_new = !self.name_terms.contains_key(token);
            self.name_terms
                .entry(token.clone())
                .or_default()
                .insert(record.id);
            if build_prefixes {
                for prefix in token_prefixes(token) {
                    self.name_prefixes
                        .entry(prefix)
                        .or_default()
                        .insert(record.id);
                }
            }
            if build_fuzzy && is_new {
                self.add_fuzzy_term(token);
            }
        }
        if build_substrings {
            for gram in substring_grams(&columns.name) {
                self.name_substrings
                    .entry(gram)
                    .or_default()
                    .insert(record.id);
            }
        }
        for token in &columns.path_tokens {
            self.path_terms
                .entry(token.clone())
                .or_default()
                .insert(record.id);
        }
        if let Some(ext) = &columns.extension {
            self.extension
                .entry(ext.clone())
                .or_default()
                .insert(record.id);
        }
        self.kind.entry(record.kind).or_default().insert(record.id);
        if build_metadata {
            for tag in &columns.tags {
                self.tags.entry(tag.clone()).or_default().insert(record.id);
            }
            for token in &columns.metadata_tokens {
                self.metadata_terms
                    .entry(token.clone())
                    .or_default()
                    .insert(record.id);
            }
        }
    }

    fn remove_terms(&mut self, record: &FileRecord) {
        let columns = self
            .columns
            .get(&record.id)
            .cloned()
            .unwrap_or_else(|| RecordColumns::from_record(record));
        remove_id(&mut self.name_exact, &columns.name, record.id);
        for gram in substring_grams(&columns.name) {
            remove_id(&mut self.name_substrings, &gram, record.id);
        }
        for token in &columns.name_tokens {
            remove_id(&mut self.name_terms, token, record.id);
            for prefix in token_prefixes(token) {
                remove_id(&mut self.name_prefixes, &prefix, record.id);
            }
            if !self.name_terms.contains_key(token) {
                self.remove_fuzzy_term(token);
            }
        }
        for token in &columns.path_tokens {
            remove_id(&mut self.path_terms, token, record.id);
        }
        if let Some(ext) = &columns.extension {
            remove_id(&mut self.extension, ext, record.id);
        }
        if let Some(ids) = self.kind.get_mut(&record.kind) {
            ids.remove(&record.id);
            if ids.is_empty() {
                self.kind.remove(&record.kind);
            }
        }
        for tag in &columns.tags {
            remove_id(&mut self.tags, tag, record.id);
        }
        for token in &columns.metadata_tokens {
            remove_id(&mut self.metadata_terms, token, record.id);
        }
        self.remove_content(record.id);
    }

    fn add_fuzzy_term(&mut self, term: &str) {
        if !is_fuzzy_term(term) {
            return;
        }
        for key in deletion_keys(term, 2) {
            self.fuzzy_terms
                .entry(key)
                .or_default()
                .insert(term.to_string());
        }
    }

    fn remove_fuzzy_term(&mut self, term: &str) {
        if !is_fuzzy_term(term) {
            return;
        }
        for key in deletion_keys(term, 2) {
            if let Some(terms) = self.fuzzy_terms.get_mut(&key) {
                terms.remove(term);
                if terms.is_empty() {
                    self.fuzzy_terms.remove(&key);
                }
            }
        }
    }
}

fn remove_id(map: &mut BTreeMap<String, BTreeSet<FileId>>, key: &str, id: FileId) {
    if let Some(ids) = map.get_mut(key) {
        ids.remove(&id);
        if ids.is_empty() {
            map.remove(key);
        }
    }
}
