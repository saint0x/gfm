use crate::query::{normalize, tokenize, SearchQuery};
use gfm_types::{FileKind, FileRecord};
use std::path::Path;
use std::time::Duration;

const APPLICATION_SCORE: i64 = 360;
const REQUESTED_LOCATION_SCORE: i64 = 260;
const SCREENSHOT_SCORE: i64 = 320;
const PROJECT_SCORE: i64 = 300;
const RECENT_SCORE: i64 = 900;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct QueryIntent {
    application: bool,
    downloads: bool,
    desktop: bool,
    screenshot: bool,
    project: bool,
    recent: bool,
}

impl QueryIntent {
    pub(crate) fn from_query(query: &SearchQuery) -> Self {
        if query.terms.is_empty() && query.phrases.is_empty() {
            return Self::default();
        }

        let terms = query.terms.iter().map(String::as_str).collect::<Vec<_>>();
        let phrases = query.phrases.iter().map(String::as_str).collect::<Vec<_>>();
        Self {
            application: has_any(
                &terms,
                &phrases,
                &["app", "apps", "application", "applications"],
            ),
            downloads: has_any(&terms, &phrases, &["download", "downloads"]),
            desktop: has_any(&terms, &phrases, &["desktop"]),
            screenshot: has_screenshot_intent(&terms, &phrases),
            project: has_any(
                &terms,
                &phrases,
                &[
                    "project",
                    "projects",
                    "repo",
                    "repository",
                    "workspace",
                    "code",
                ],
            ),
            recent: has_any(
                &terms,
                &phrases,
                &["recent", "recents", "recently", "today", "latest", "new"],
            ),
        }
    }

    pub(crate) fn is_empty(self) -> bool {
        !self.application
            && !self.downloads
            && !self.desktop
            && !self.screenshot
            && !self.project
            && !self.recent
    }

    pub(crate) fn score(self, record: &FileRecord) -> i64 {
        let mut score = 0;
        if self.application && is_application(record) {
            score += APPLICATION_SCORE;
        }
        if self.downloads && has_component(&record.path, &["downloads"]) {
            score += REQUESTED_LOCATION_SCORE;
        }
        if self.desktop && has_component(&record.path, &["desktop"]) {
            score += REQUESTED_LOCATION_SCORE;
        }
        if self.screenshot && is_screenshot(record) {
            score += SCREENSHOT_SCORE;
            if has_component(&record.path, &["desktop"]) {
                score += 40;
            }
        }
        if self.project && is_project_folder(record) {
            score += PROJECT_SCORE;
        }
        if self.recent {
            score += recent_score(record);
        }
        score
    }
}

pub(crate) fn term_matches_intent(term: &str, record: &FileRecord) -> bool {
    match term {
        "app" | "apps" | "application" | "applications" => is_application(record),
        "download" | "downloads" => has_component(&record.path, &["downloads"]),
        "desktop" => has_component(&record.path, &["desktop"]),
        "screenshot" | "screenshots" | "capture" => is_screenshot(record),
        "project" | "projects" | "repo" | "repository" | "workspace" | "code" => {
            is_project_folder(record)
        }
        "recent" | "recents" | "recently" | "today" | "latest" | "new" => recent_score(record) > 0,
        _ => false,
    }
}

fn has_any(terms: &[&str], phrases: &[&str], needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        terms.contains(needle) || phrases.iter().any(|phrase| phrase.contains(needle))
    })
}

fn has_screenshot_intent(terms: &[&str], phrases: &[&str]) -> bool {
    has_any(terms, phrases, &["screenshot", "screenshots", "capture"])
        || (terms.contains(&"screen") && terms.contains(&"shot"))
        || phrases.iter().any(|phrase| phrase.contains("screen shot"))
}

fn is_application(record: &FileRecord) -> bool {
    record
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        || has_component(&record.path, &["applications"])
}

fn is_screenshot(record: &FileRecord) -> bool {
    let name = normalize(&record.name);
    name.contains("screenshot")
        || name.contains("screen shot")
        || (name.contains("screen") && name.contains("capture"))
}

fn is_project_folder(record: &FileRecord) -> bool {
    record.kind == FileKind::Directory
        && !is_generic_folder_name(&record.name)
        && has_component(
            &record.path,
            &[
                "developer",
                "development",
                "dev",
                "code",
                "projects",
                "repos",
                "workspace",
                "work",
            ],
        )
}

fn is_generic_folder_name(name: &str) -> bool {
    matches!(
        normalize(name).as_str(),
        "desktop"
            | "documents"
            | "downloads"
            | "applications"
            | "library"
            | "users"
            | "home"
            | "work"
            | "projects"
            | "repos"
            | "code"
            | "src"
    )
}

fn recent_score(record: &FileRecord) -> i64 {
    let Some(modified) = record.modified else {
        return 0;
    };
    let Ok(age) = modified.elapsed() else {
        return RECENT_SCORE;
    };
    if age <= Duration::from_secs(86_400) {
        RECENT_SCORE
    } else if age <= Duration::from_secs(7 * 86_400) {
        220
    } else if age <= Duration::from_secs(30 * 86_400) {
        150
    } else {
        0
    }
}

fn has_component(path: &Path, components: &[&str]) -> bool {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .flat_map(|component| tokenize(&normalize(component)))
        .any(|component| components.contains(&component.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SearchQuery;
    use gfm_types::{FileId, VolumeId};
    use std::path::PathBuf;

    #[test]
    fn ordinary_queries_do_not_request_intent_scan() {
        let intent = QueryIntent::from_query(&SearchQuery::parse("quarterly report"));

        assert!(intent.is_empty());
    }

    #[test]
    fn finder_intent_queries_score_matching_records() {
        let intent = QueryIntent::from_query(&SearchQuery::parse("recent applications"));
        let mut record = record("/Applications/Notes.app", "Notes.app");
        record.kind = FileKind::Directory;
        record.modified = Some(std::time::SystemTime::now());

        assert!(!intent.is_empty());
        assert!(intent.score(&record) > APPLICATION_SCORE);
    }

    fn record(path: &str, name: &str) -> FileRecord {
        FileRecord {
            id: FileId::new(VolumeId(1), 1),
            parent: None,
            path: PathBuf::from(path),
            name: name.to_string(),
            kind: FileKind::File,
            len: 0,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        }
    }
}
