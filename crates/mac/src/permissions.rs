use gfm_types::{GfmError, Result};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPromptMode {
    DeferUntilNeeded,
    ExplainOnly,
    RequireBeforeIndexing,
}

impl PermissionPromptMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeferUntilNeeded => "defer-until-needed",
            Self::ExplainOnly => "explain-only",
            Self::RequireBeforeIndexing => "require-before-indexing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPolicy {
    pub prompt_mode: PermissionPromptMode,
    pub require_full_disk_access: bool,
    pub allow_degraded_machine_search: bool,
    pub finder_parity_default: bool,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            prompt_mode: PermissionPromptMode::DeferUntilNeeded,
            require_full_disk_access: false,
            allow_degraded_machine_search: true,
            finder_parity_default: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionScope {
    Desktop,
    Documents,
    Downloads,
    Mail,
    Photos,
    FullDiskAccess,
}

impl PermissionScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Documents => "documents",
            Self::Downloads => "downloads",
            Self::Mail => "mail",
            Self::Photos => "photos",
            Self::FullDiskAccess => "full-disk-access",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    Granted,
    Denied,
    Missing,
    Unknown,
}

impl PermissionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Denied => "denied",
            Self::Missing => "missing",
            Self::Unknown => "unknown",
        }
    }

    const fn is_granted(self) -> bool {
        matches!(self, Self::Granted | Self::Missing)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionReadiness {
    pub scope: PermissionScope,
    pub path: PathBuf,
    pub state: PermissionState,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionAction {
    ContinueNormally,
    ContinueDegraded,
    ExplainFullDiskAccess,
    BlockUntilGranted,
}

impl PermissionAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ContinueNormally => "continue-normally",
            Self::ContinueDegraded => "continue-degraded",
            Self::ExplainFullDiskAccess => "explain-full-disk-access",
            Self::BlockUntilGranted => "block-until-granted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOnboardingPlan {
    pub policy: PermissionPolicy,
    pub readiness: Vec<PermissionReadiness>,
    pub action: PermissionAction,
    pub finder_parity_default: bool,
}

impl PermissionOnboardingPlan {
    pub fn denied_scopes(&self) -> impl Iterator<Item = &PermissionReadiness> {
        self.readiness
            .iter()
            .filter(|item| item.state == PermissionState::Denied)
    }

    pub fn granted_for_machine_search(&self) -> bool {
        self.readiness
            .iter()
            .filter(|item| item.scope != PermissionScope::FullDiskAccess)
            .all(|item| item.state.is_granted())
    }
}

pub fn current_permission_onboarding() -> Result<PermissionOnboardingPlan> {
    permission_onboarding(PermissionPolicy::default(), default_permission_roots()?)
}

pub fn permission_onboarding(
    policy: PermissionPolicy,
    roots: Vec<(PermissionScope, PathBuf)>,
) -> Result<PermissionOnboardingPlan> {
    let readiness = roots
        .into_iter()
        .map(|(scope, path)| probe_scope(scope, path))
        .collect::<Vec<_>>();
    let denied = readiness
        .iter()
        .any(|item| item.state == PermissionState::Denied);
    let full_disk_denied = readiness.iter().any(|item| {
        item.scope == PermissionScope::FullDiskAccess && item.state == PermissionState::Denied
    });
    let action = if denied && policy.require_full_disk_access {
        PermissionAction::BlockUntilGranted
    } else if full_disk_denied && matches!(policy.prompt_mode, PermissionPromptMode::ExplainOnly) {
        PermissionAction::ExplainFullDiskAccess
    } else if denied && policy.allow_degraded_machine_search {
        PermissionAction::ContinueDegraded
    } else if denied {
        PermissionAction::BlockUntilGranted
    } else {
        PermissionAction::ContinueNormally
    };

    Ok(PermissionOnboardingPlan {
        finder_parity_default: policy.finder_parity_default
            && matches!(policy.prompt_mode, PermissionPromptMode::DeferUntilNeeded)
            && !policy.require_full_disk_access,
        policy,
        readiness,
        action,
    })
}

fn default_permission_roots() -> Result<Vec<(PermissionScope, PathBuf)>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| GfmError::Format("HOME is not set".to_string()))?;
    Ok(vec![
        (PermissionScope::Desktop, home.join("Desktop")),
        (PermissionScope::Documents, home.join("Documents")),
        (PermissionScope::Downloads, home.join("Downloads")),
        (
            PermissionScope::Mail,
            home.join("Library/Group Containers/group.com.apple.mail"),
        ),
        (
            PermissionScope::Photos,
            home.join("Pictures/Photos Library.photoslibrary"),
        ),
        (PermissionScope::FullDiskAccess, home.join("Library/Mail")),
    ])
}

fn probe_scope(scope: PermissionScope, path: PathBuf) -> PermissionReadiness {
    match fs::read_dir(&path) {
        Ok(_) => PermissionReadiness {
            scope,
            path,
            state: PermissionState::Granted,
            reason: "readable".to_string(),
        },
        Err(err) if err.kind() == ErrorKind::NotFound => PermissionReadiness {
            scope,
            path,
            state: PermissionState::Missing,
            reason: "path is not present on this host".to_string(),
        },
        Err(err) if err.kind() == ErrorKind::PermissionDenied => PermissionReadiness {
            scope,
            path,
            state: PermissionState::Denied,
            reason: "macOS denied read access".to_string(),
        },
        Err(err) => PermissionReadiness {
            scope,
            path,
            state: PermissionState::Unknown,
            reason: format!("probe failed: {err}"),
        },
    }
}

#[allow(dead_code)]
fn probe_file(path: &Path) -> PermissionState {
    match fs::metadata(path) {
        Ok(_) => PermissionState::Granted,
        Err(err) if err.kind() == ErrorKind::NotFound => PermissionState::Missing,
        Err(err) if err.kind() == ErrorKind::PermissionDenied => PermissionState::Denied,
        Err(_) => PermissionState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn default_policy_preserves_finder_parity_and_degraded_search() {
        let root = temp_root("permissions-ok");
        let desktop = root.join("Desktop");
        fs::create_dir_all(&desktop).unwrap();
        let plan = permission_onboarding(
            PermissionPolicy::default(),
            vec![(PermissionScope::Desktop, desktop)],
        )
        .unwrap();

        assert_eq!(plan.action, PermissionAction::ContinueNormally);
        assert!(plan.finder_parity_default);
        assert!(plan.granted_for_machine_search());
    }

    #[test]
    fn missing_optional_roots_do_not_block_machine_search() {
        let root = temp_root("permissions-missing");
        let plan = permission_onboarding(
            PermissionPolicy::default(),
            vec![(
                PermissionScope::Photos,
                root.join("Photos Library.photoslibrary"),
            )],
        )
        .unwrap();

        assert_eq!(plan.action, PermissionAction::ContinueNormally);
        assert!(plan.granted_for_machine_search());
        assert_eq!(plan.readiness[0].state, PermissionState::Missing);
    }

    #[test]
    fn denied_roots_choose_degraded_mode_by_default() {
        let policy = PermissionPolicy::default();
        let plan = PermissionOnboardingPlan {
            policy: policy.clone(),
            readiness: vec![PermissionReadiness {
                scope: PermissionScope::Documents,
                path: PathBuf::from("/private"),
                state: PermissionState::Denied,
                reason: "macOS denied read access".to_string(),
            }],
            action: if policy.allow_degraded_machine_search {
                PermissionAction::ContinueDegraded
            } else {
                PermissionAction::BlockUntilGranted
            },
            finder_parity_default: true,
        };

        assert_eq!(plan.action, PermissionAction::ContinueDegraded);
        assert_eq!(plan.denied_scopes().count(), 1);
    }

    #[test]
    fn required_full_disk_access_blocks_when_denied() {
        let root = temp_root("permissions-fda");
        let policy = PermissionPolicy {
            require_full_disk_access: true,
            ..PermissionPolicy::default()
        };
        let plan = PermissionOnboardingPlan {
            policy,
            readiness: vec![PermissionReadiness {
                scope: PermissionScope::FullDiskAccess,
                path: root.join("Library/Mail"),
                state: PermissionState::Denied,
                reason: "macOS denied read access".to_string(),
            }],
            action: PermissionAction::BlockUntilGranted,
            finder_parity_default: false,
        };

        assert_eq!(plan.action, PermissionAction::BlockUntilGranted);
        assert!(!plan.finder_parity_default);
    }

    fn temp_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gfm-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
