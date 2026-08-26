use crate::access::destination_probe_path;
use crate::{OperationAccessRequirement, OperationAccessRole};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Copy { from: PathBuf, to: PathBuf },
    Move { from: PathBuf, to: PathBuf },
    Rename { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf },
    Trash { path: PathBuf },
    EmptyTrash { path: PathBuf },
    Restore { from: PathBuf, to: PathBuf },
}

impl Operation {
    pub fn target_path(&self) -> Option<&Path> {
        match self {
            Self::Copy { to, .. }
            | Self::Move { to, .. }
            | Self::Rename { to, .. }
            | Self::Restore { to, .. } => Some(to),
            Self::Delete { .. } | Self::Trash { .. } | Self::EmptyTrash { .. } => None,
        }
    }

    pub fn access_requirements(&self) -> Vec<OperationAccessRequirement> {
        match self {
            Self::Copy { from, to }
            | Self::Move { from, to }
            | Self::Rename { from, to }
            | Self::Restore { from, to } => vec![
                OperationAccessRequirement {
                    path: from.clone(),
                    role: OperationAccessRole::Source,
                },
                OperationAccessRequirement {
                    path: destination_probe_path(to),
                    role: OperationAccessRole::DestinationParent,
                },
            ],
            Self::Delete { path } | Self::Trash { path } | Self::EmptyTrash { path } => {
                vec![OperationAccessRequirement {
                    path: path.clone(),
                    role: OperationAccessRole::Target,
                }]
            }
        }
    }
}
