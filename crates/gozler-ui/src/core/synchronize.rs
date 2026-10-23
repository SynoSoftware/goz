//! The comparison dialog retains only the queue id and its immutable preview.

use super::text::{Numbers, Strings};
use gozler_fileops::{Comparison, OperationId, SyncAction};
use std::sync::Arc;

#[derive(Default)]
pub(crate) enum Preview {
    #[default]
    Empty,
    Comparing(OperationId),
    Ready {
        id: OperationId,
        comparison: Arc<Comparison>,
    },
    Failed(OperationId),
}

impl Preview {
    pub(crate) fn id(&self) -> Option<OperationId> {
        match self {
            Self::Empty => None,
            Self::Comparing(id) | Self::Failed(id) | Self::Ready { id, .. } => Some(*id),
        }
    }

    pub(crate) fn comparison(&self) -> Option<&Comparison> {
        match self {
            Self::Ready { comparison, .. } => Some(comparison),
            _ => None,
        }
    }

    pub(crate) fn can_commit(&self) -> bool {
        self.comparison()
            .is_some_and(|comparison| comparison.summary().conflicts == 0)
    }
}

pub(crate) fn action(strings: &Strings, action: SyncAction) -> String {
    strings
        .text(match action {
            SyncAction::Add => "sync.add",
            SyncAction::Replace => "sync.replace",
            SyncAction::Conflict => "sync.conflict",
            SyncAction::Unchanged => "sync.unchanged",
            SyncAction::Keep => "sync.keep",
            SyncAction::Delete => "file_operations.delete",
        })
        .to_owned()
}

pub(crate) fn summary(strings: &Strings, numbers: &dyn Numbers, comparison: &Comparison) -> String {
    let summary = comparison.summary();
    let counts = [
        summary.additions,
        summary.replacements,
        summary.unchanged,
        summary.extras,
        summary.conflicts,
    ]
    .map(|count| numbers.integer(count));
    strings.format(
        "sync.summary",
        &counts.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}
