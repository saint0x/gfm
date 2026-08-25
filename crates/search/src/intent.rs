use crate::query::{normalize, tokenize, SearchQuery};
use gfm_types::{FileKind, FileRecord};
use std::path::Path;
use std::time::Duration;

const APPLICATION_SCORE: i64 = 360;
const REQUESTED_LOCATION_SCORE: i64 = 260;
const SCREENSHOT_SCORE: i64 = 320;
const PROJECT_SCORE: i64 = 300;
const RECENT_SCORE: i64 = 900;

pub(crate) fn intent_score(query: &SearchQuery, record: &FileRecord) -> i64 {
    if query.terms.is_empty() && query.phrases.is_empty() {
        return 0;
    }

    let mut score = 0;
    let terms = query.terms.iter().map(String::as_str).collect::<Vec<_>>();
    let phrases = query.phrases.iter().map(String::as_str).collect::<Vec<_>>();

    if has_any(
        &terms,
        &phrases,
        &["app", "apps", "application", "applications"],
    ) && is_application(record)
    {
        score += APPLICATION_SCORE;
    }

    if has_any(&terms, &phrases, &["download", "downloads"])
        && has_component(&record.path, &["downloads"])
    {
        score += REQUESTED_LOCATION_SCORE;
    }

    if has_any(&terms, &phrases, &["desktop"]) && has_component(&record.path, &["desktop"]) {
        score += REQUESTED_LOCATION_SCORE;
    }

    if has_screenshot_intent(&terms, &phrases) && is_screenshot(record) {
        score += SCREENSHOT_SCORE;
        if has_component(&record.path, &["desktop"]) {
            score += 40;
        }
    }

    if has_any(
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
    ) && is_project_folder(record)
    {
        score += PROJECT_SCORE;
    }

    if has_any(
        &terms,
        &phrases,
        &["recent", "recents", "recently", "today", "latest", "new"],
    ) {
        score += recent_score(record);
    }

    score
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
