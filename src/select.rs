use clap::ValueEnum;

use crate::model::{DupeFile, DupeGroup};

/// How to pick the copy to keep inside a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeepStrategy {
    /// Keep the oldest copy (the original, usually).
    Oldest,
    /// Keep the newest copy.
    Newest,
    /// Keep the copy closest to the scan root.
    Shallowest,
    /// Keep the deepest copy.
    Deepest,
    /// Keep the lexicographically first path.
    First,
}

impl KeepStrategy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Oldest => "oldest",
            Self::Newest => "newest",
            Self::Shallowest => "shallowest",
            Self::Deepest => "deepest",
            Self::First => "first",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Oldest => Self::Newest,
            Self::Newest => Self::Shallowest,
            Self::Shallowest => Self::Deepest,
            Self::Deepest => Self::First,
            Self::First => Self::Oldest,
        }
    }
}

impl std::fmt::Display for KeepStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Markers that suggest a file is the disposable copy rather than the original.
const COPY_HINTS: &[&str] = &[
    "copy",
    "duplicate",
    "backup",
    "conflicted",
    " (1)",
    " (2)",
    " 2.",
];

fn penalty(file: &DupeFile) -> u8 {
    let path = file.path.to_string_lossy().to_lowercase();
    COPY_HINTS
        .iter()
        .filter(|hint| path.contains(*hint))
        .count()
        .min(255) as u8
}

fn depth(file: &DupeFile) -> usize {
    file.path.components().count()
}

/// Lower sorts better.
fn score(file: &DupeFile, strategy: KeepStrategy) -> (u8, i64, i64, usize) {
    let mtime = file.modified as i64;
    let mtime_rank = match strategy {
        KeepStrategy::Newest => -mtime,
        _ => mtime,
    };
    let depth_rank = match strategy {
        KeepStrategy::Shallowest => depth(file) as i64,
        KeepStrategy::Deepest => -(depth(file) as i64),
        _ => 0,
    };
    (
        penalty(file),
        mtime_rank,
        depth_rank,
        file.path.as_os_str().len(),
    )
}

/// Index of the file worth keeping.
pub fn keep_index(group: &DupeGroup, strategy: KeepStrategy) -> usize {
    debug_assert!(!group.files.is_empty());
    if strategy == KeepStrategy::First {
        let mut best = 0;
        for (i, f) in group.files.iter().enumerate() {
            if f.path.cmp(&group.files[best].path).is_lt() {
                best = i;
            }
        }
        return best;
    }
    (0..group.files.len())
        .min_by_key(|&i| score(&group.files[i], strategy))
        .unwrap_or(0)
}

/// One flag per file: `true` means "this copy can go".
pub fn mark_victims(group: &DupeGroup, strategy: KeepStrategy) -> Vec<bool> {
    let keep = keep_index(group, strategy);
    let mut marks = vec![true; group.files.len()];
    if let Some(slot) = marks.get_mut(keep) {
        *slot = false;
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file(path: &str, mtime: u64) -> DupeFile {
        DupeFile {
            path: PathBuf::from(path),
            size: 10,
            modified: mtime,
        }
    }

    fn group() -> DupeGroup {
        DupeGroup {
            size: 10,
            hash: "h".into(),
            files: vec![
                file("/Users/me/Photos/IMG_001.HEIC", 1_700_000_000),
                file("/Users/me/Downloads/IMG_001 copy.HEIC", 1_800_000_000),
                file("/Users/me/Photos/IMG_001 2.HEIC", 1_650_000_000),
            ],
        }
    }

    #[test]
    fn oldest_keeps_original_not_copy() {
        let g = group();
        assert_eq!(keep_index(&g, KeepStrategy::Oldest), 0);
    }

    #[test]
    fn newest_prefers_newer_but_still_penalises_copies() {
        let g = group();
        // The copy is newest, but the copy penalty outranks recency.
        assert_eq!(keep_index(&g, KeepStrategy::Newest), 0);
    }

    #[test]
    fn victims_never_include_the_keeper() {
        let g = group();
        for strategy in [
            KeepStrategy::Oldest,
            KeepStrategy::Newest,
            KeepStrategy::Shallowest,
            KeepStrategy::Deepest,
            KeepStrategy::First,
        ] {
            let marks = mark_victims(&g, strategy);
            assert_eq!(marks.iter().filter(|m| !**m).count(), 1, "{strategy:?}");
        }
    }

    #[test]
    fn shallowest_beats_deeper_on_tie() {
        let g = DupeGroup {
            size: 10,
            hash: "h".into(),
            files: vec![file("/a/b/c/d/e/f.bin", 100), file("/a/f.bin", 100)],
        };
        assert_eq!(keep_index(&g, KeepStrategy::Shallowest), 1);
    }
}
