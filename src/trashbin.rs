use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// One file scheduled for removal, with the size we expect to free.
#[derive(Debug, Clone)]
pub struct DeleteFile {
    pub path: PathBuf,
    pub size: u64,
}

/// Snapshot published after each batch so the status bar can move.
#[derive(Debug, Clone, Copy)]
pub struct DeleteProgress {
    pub completed: usize,
    pub total: usize,
    pub freed: u64,
}

#[derive(Debug, Default)]
pub struct DeleteOutcome {
    pub deleted: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
    pub freed: u64,
    pub cancelled: bool,
}

impl DeleteOutcome {
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty() && !self.cancelled
    }
}

/// Finder AppleScript has a command-length limit, so Trash goes out in batches.
const TRASH_CHUNK: usize = 200;

/// Move files to the macOS Trash, one Finder call per chunk.
///
/// `on_progress` runs after each chunk. `stop` is checked between chunks, so a
/// cancel finishes the batch already sent to Finder.
pub fn trash_files(
    files: &[DeleteFile],
    stop: &AtomicBool,
    on_progress: impl FnMut(DeleteProgress),
) -> DeleteOutcome {
    let outcome = run_chunks(files, stop, trash_chunk, on_progress);
    if !outcome.deleted.is_empty() {
        let _ = write_manifest("trash", &outcome.deleted);
    }
    outcome
}

/// Permanently delete files. Empty directories are removed only inside `roots`.
pub fn purge_files(
    files: &[DeleteFile],
    roots: &[PathBuf],
    stop: &AtomicBool,
    on_progress: impl FnMut(DeleteProgress),
) -> DeleteOutcome {
    let outcome = run_chunks(files, stop, purge_chunk, on_progress);
    if !outcome.deleted.is_empty() {
        let _ = write_manifest("purge", &outcome.deleted);
        prune_empty_dirs(&outcome.deleted, roots);
    }
    outcome
}

fn run_chunks(
    files: &[DeleteFile],
    stop: &AtomicBool,
    mut op: impl FnMut(&[DeleteFile], &mut DeleteOutcome),
    mut on_progress: impl FnMut(DeleteProgress),
) -> DeleteOutcome {
    let total = files.len();
    let mut outcome = DeleteOutcome::default();
    let mut completed = 0usize;
    for chunk in files.chunks(TRASH_CHUNK) {
        if stop.load(Ordering::Acquire) {
            outcome.cancelled = true;
            break;
        }
        op(chunk, &mut outcome);
        completed += chunk.len();
        on_progress(DeleteProgress {
            completed,
            total,
            freed: outcome.freed,
        });
    }
    outcome
}

fn trash_chunk(files: &[DeleteFile], outcome: &mut DeleteOutcome) {
    trash_split(files, outcome);
}

/// One bad path fails the whole Finder call, so split until the failure is alone.
fn trash_split(files: &[DeleteFile], outcome: &mut DeleteOutcome) {
    if files.is_empty() {
        return;
    }
    let paths: Vec<&Path> = files.iter().map(|file| file.path.as_path()).collect();
    match trash::delete_all(paths) {
        Ok(()) => record_deleted(files, outcome),
        Err(err) if files.len() == 1 => {
            outcome
                .failed
                .push((files[0].path.clone(), err.to_string()));
        }
        Err(_) => {
            let mid = files.len() / 2;
            trash_split(&files[..mid], outcome);
            trash_split(&files[mid..], outcome);
        }
    }
}

fn purge_chunk(files: &[DeleteFile], outcome: &mut DeleteOutcome) {
    let part = if files.len() < 32 {
        sequential_remove(files)
    } else {
        parallel_remove(files)
    };
    outcome.freed = outcome.freed.saturating_add(part.freed);
    outcome.deleted.extend(part.deleted);
    outcome.failed.extend(part.failed);
}

fn parallel_remove(files: &[DeleteFile]) -> DeleteOutcome {
    let threads = 8.min(files.len());
    if threads <= 1 {
        return sequential_remove(files);
    }
    let size = files.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for part in files.chunks(size) {
            handles.push(scope.spawn(|| sequential_remove(part)));
        }
        handles
            .into_iter()
            .fold(DeleteOutcome::default(), |mut acc, handle| {
                let part = handle.join().unwrap_or_default();
                acc.freed = acc.freed.saturating_add(part.freed);
                acc.deleted.extend(part.deleted);
                acc.failed.extend(part.failed);
                acc
            })
    })
}

fn sequential_remove(files: &[DeleteFile]) -> DeleteOutcome {
    let mut outcome = DeleteOutcome::default();
    for file in files {
        match std::fs::remove_file(&file.path) {
            Ok(()) => {
                outcome.deleted.push(file.path.clone());
                outcome.freed = outcome.freed.saturating_add(file.size);
            }
            Err(err) => {
                let message = if err.kind() == ErrorKind::NotFound {
                    "file disappeared".to_string()
                } else {
                    err.to_string()
                };
                outcome.failed.push((file.path.clone(), message));
            }
        }
    }
    outcome
}

fn record_deleted(files: &[DeleteFile], outcome: &mut DeleteOutcome) {
    for file in files {
        outcome.deleted.push(file.path.clone());
        outcome.freed = outcome.freed.saturating_add(file.size);
    }
}

#[derive(Serialize)]
struct ManifestEntry<'a> {
    path: &'a Path,
    mode: &'a str,
    at: u64,
}

/// Record paths that were actually removed.
fn write_manifest(mode: &str, paths: &[PathBuf]) -> Option<PathBuf> {
    let dir = dirs::data_dir()?.join("kembar").join("history");
    std::fs::create_dir_all(&dir).ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    let entries: Vec<ManifestEntry<'_>> = paths
        .iter()
        .map(|path| ManifestEntry {
            path: path.as_path(),
            mode,
            at: now.as_secs(),
        })
        .collect();
    let file = dir.join(format!("{}-{mode}.json", now.as_millis()));
    let json = serde_json::to_string(&entries).ok()?;
    std::fs::write(&file, json).ok()?;
    Some(file)
}

/// Remove directories the purge just emptied. Never removes a scan root, and
/// never walks above one.
fn prune_empty_dirs(removed: &[PathBuf], roots: &[PathBuf]) {
    if roots.is_empty() {
        return;
    }
    let mut parents = HashSet::new();
    for file in removed {
        let mut dir = file.parent().map(Path::to_path_buf);
        while let Some(current) = dir {
            if !is_inside_root(&current, roots) {
                break;
            }
            let parent = current.parent().map(Path::to_path_buf);
            parents.insert(current);
            dir = parent;
        }
    }
    let mut parents: Vec<PathBuf> = parents.into_iter().collect();
    parents.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for dir in parents {
        let empty = std::fs::read_dir(&dir)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false);
        if empty {
            let _ = std::fs::remove_dir(&dir);
        }
    }
}

fn is_inside_root(dir: &Path, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .any(|root| dir != root.as_path() && dir.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch() -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("kembar-prune-{n}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prune_removes_empty_child_and_keeps_the_scan_root() {
        let root = scratch();
        let nested = root.join("album").join("day");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("a.jpg");
        fs::write(&file, b"x").unwrap();
        fs::remove_file(&file).unwrap();

        prune_empty_dirs(&[file], &[root.clone()]);

        assert!(root.exists());
        assert!(!root.join("album").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn prune_keeps_a_directory_that_still_has_a_file() {
        let root = scratch();
        let dir = root.join("album");
        fs::create_dir_all(&dir).unwrap();
        let gone = dir.join("a.jpg");
        let stay = dir.join("b.jpg");
        fs::write(&gone, b"x").unwrap();
        fs::write(&stay, b"y").unwrap();
        fs::remove_file(&gone).unwrap();

        prune_empty_dirs(&[gone], &[root.clone()]);

        assert!(stay.exists());
        assert!(dir.exists());
        assert!(root.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_file_is_reported_without_a_prior_stat() {
        let root = scratch();
        let missing = root.join("gone.jpg");
        let outcome = sequential_remove(&[DeleteFile {
            path: missing,
            size: 4,
        }]);
        assert!(outcome.deleted.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].1, "file disappeared");
        assert_eq!(outcome.freed, 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn purge_chunk_removes_a_batch_together() {
        let root = scratch();
        let files: Vec<DeleteFile> = (0..40)
            .map(|i| {
                let path = root.join(format!("{i}.bin"));
                fs::write(&path, b"x").unwrap();
                DeleteFile { path, size: 1 }
            })
            .collect();
        let mut outcome = DeleteOutcome::default();
        purge_chunk(&files, &mut outcome);
        assert_eq!(outcome.deleted.len(), 40);
        assert!(outcome.failed.is_empty());
        assert_eq!(outcome.freed, 40);
        assert!(fs::read_dir(&root).unwrap().next().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stop_flag_skips_chunks_after_the_current_one() {
        let files: Vec<DeleteFile> = (0..TRASH_CHUNK + 10)
            .map(|i| DeleteFile {
                path: PathBuf::from(format!("/tmp/kembar-{i}")),
                size: 1,
            })
            .collect();
        let stop = AtomicBool::new(false);
        let mut calls = 0usize;
        let outcome = run_chunks(
            &files,
            &stop,
            |chunk, outcome| {
                calls += 1;
                stop.store(true, Ordering::Release);
                record_deleted(chunk, outcome);
            },
            |_| {},
        );
        assert!(outcome.cancelled);
        assert_eq!(calls, 1);
        assert_eq!(outcome.deleted.len(), TRASH_CHUNK);
        assert_eq!(outcome.freed, TRASH_CHUNK as u64);
    }
}
