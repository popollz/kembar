use std::path::PathBuf;

use serde::Serialize;

/// One physical file inside a duplicate group.
#[derive(Debug, Clone, Serialize)]
pub struct DupeFile {
    pub path: PathBuf,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: u64,
}

impl DupeFile {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }

    pub fn parent(&self) -> String {
        self.path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Files that are byte-for-byte identical.
#[derive(Debug, Clone, Serialize)]
pub struct DupeGroup {
    pub size: u64,
    pub hash: String,
    pub files: Vec<DupeFile>,
}

impl DupeGroup {
    /// Space recovered if every copy but one is removed.
    pub fn reclaimable(&self) -> u64 {
        self.size
            .saturating_mul(self.files.len().saturating_sub(1) as u64)
    }

    pub fn is_trivial(&self) -> bool {
        self.files.len() < 2
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanReport {
    pub roots: Vec<PathBuf>,
    pub groups: Vec<DupeGroup>,
    pub elapsed_ms: u128,
    pub reclaimable: u64,
    pub duplicate_files: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

impl ScanReport {
    pub fn rebuild_totals(&mut self) {
        self.groups.retain(|g| !g.is_trivial());
        self.reclaimable = self.groups.iter().map(DupeGroup::reclaimable).sum();
        self.duplicate_files = self.groups.iter().map(|g| g.files.len() - 1).sum();
    }
}
