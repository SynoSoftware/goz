//! A synchronization compares complete trees before committing changes. The
//! queue retains the reviewed intent; the window reads individual preview rows.

#[cfg(windows)]
pub(crate) mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncAction {
    Add,
    Replace,
    Conflict,
    Unchanged,
    Keep,
    Delete,
}

#[derive(Debug)]
pub struct ComparisonEntry {
    pub path: String,
    pub action: SyncAction,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ComparisonSummary {
    pub additions: u64,
    pub replacements: u64,
    pub conflicts: u64,
    pub unchanged: u64,
    pub extras: u64,
    pub deletions: u64,
}

#[derive(Debug)]
pub struct Comparison {
    entries: Vec<ComparisonEntry>,
    summary: ComparisonSummary,
}

impl Comparison {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entry(&self, index: usize) -> Option<&ComparisonEntry> {
        self.entries.get(index)
    }

    pub fn summary(&self) -> ComparisonSummary {
        self.summary
    }
}
