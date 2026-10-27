use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Instant;

use crossbeam_channel::{Sender, bounded};
use czkawka_core::common::model::{CheckingMethod, HashType};
use czkawka_core::common::progress_data::ProgressData;
use czkawka_core::common::tool_data::CommonData;
use czkawka_core::common::traits::Search;
use czkawka_core::tools::duplicate::{DuplicateFinder, DuplicateFinderParameters};

use crate::model::{DupeFile, DupeGroup, ScanReport};

/// Directories skipped by default: VCS/build noise, macOS metadata, and
/// package formats that manage their own contents.
pub const DEFAULT_EXCLUDES: &[&str] = &[
    "*/.git/*",
    "*/node_modules/*",
    "*/.Trash/*",
    "*/.Spotlight-V100/*",
    "*/.fseventsd/*",
    "*/.DocumentRevisions-V100/*",
    "*/Library/Caches/*",
    "*.app/*",
    "*.photoslibrary/*",
    "*.fcpbundle/*",
    "*.logicx/*",
];

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub roots: Vec<PathBuf>,
    /// Skip files smaller than this (bytes). Minimum 1.
    pub min_size: u64,
    /// Empty means "every extension".
    pub extensions: Vec<String>,
    /// Extra excluded-item globs, on top of [`DEFAULT_EXCLUDES`].
    pub extra_excludes: Vec<String>,
    pub use_cache: bool,
}

impl ScanOptions {
    pub fn excluded(&self) -> Vec<String> {
        DEFAULT_EXCLUDES
            .iter()
            .map(|s| (*s).to_string())
            .chain(self.extra_excludes.iter().cloned())
            .collect()
    }
}

#[derive(Debug)]
pub enum ScanMsg {
    Progress { pct: Option<u8>, label: String },
    Done(Box<ScanReport>),
    Failed(String),
}

/// Blocking scan. Meant to run on a worker thread; progress is streamed to `tx`.
pub fn run(opts: &ScanOptions, tx: Sender<ScanMsg>, stop: Arc<AtomicBool>) -> ScanReport {
    // czkawka_core panics if its cache/config locations were never initialised.
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = czkawka_core::common::config_cache_path::set_config_cache_path("kembar", "kembar");
    });

    let start = Instant::now();
    let mut report = ScanReport {
        roots: opts.roots.clone(),
        ..ScanReport::default()
    };

    if opts.roots.is_empty() {
        report.errors.push("no directories to scan".into());
        return report;
    }

    let params = DuplicateFinderParameters::new(
        CheckingMethod::Hash,
        HashType::Blake3,
        true,
        1024 * 1024,
        1024 * 1024,
        true,
    );
    let mut finder = DuplicateFinder::new(params);
    finder.set_included_paths(opts.roots.clone());
    finder.set_excluded_items(opts.excluded());
    if !opts.extensions.is_empty() {
        finder.set_allowed_extensions(opts.extensions.clone());
    }
    finder.set_minimal_file_size(opts.min_size.max(1));
    finder.set_maximal_file_size(u64::MAX);
    finder.set_recursive_search(true);
    finder.set_use_cache(opts.use_cache);
    // Hard links are the same bytes on disk, not waste.
    finder.set_hide_hard_links(true);

    let (progress_tx, progress_rx) = bounded::<ProgressData>(256);
    let forwarder = {
        let tx = tx.clone();
        thread::spawn(move || {
            while let Ok(progress) = progress_rx.recv() {
                let display = progress.to_display();
                let pct = u8::try_from(display.all_progress).ok().map(|p| p.min(100));
                let _ = tx.send(ScanMsg::Progress {
                    pct,
                    label: display.label,
                });
            }
        })
    };

    finder.search(&stop, Some(&progress_tx));
    drop(progress_tx);
    let _ = forwarder.join();

    let messages = finder.get_text_messages();
    if let Some(critical) = &messages.critical {
        report.errors.push(critical.clone());
    }
    report
        .errors
        .extend(messages.errors.iter().take(20).cloned());
    report
        .warnings
        .extend(messages.warnings.iter().take(20).cloned());

    if stop.load(std::sync::atomic::Ordering::Relaxed) {
        report.warnings.push("scan cancelled".into());
        return report;
    }

    for hash_groups in finder.get_files_sorted_by_hash().values() {
        for group in hash_groups {
            if group.len() < 2 {
                continue;
            }
            let files: Vec<DupeFile> = group
                .iter()
                .map(|entry| DupeFile {
                    path: entry.path.clone(),
                    size: entry.size,
                    modified: entry.modified_date,
                })
                .collect();
            report.groups.push(DupeGroup {
                size: files[0].size,
                hash: group[0].hash.clone(),
                files,
            });
        }
    }

    report
        .groups
        .sort_by_key(|group| std::cmp::Reverse(group.reclaimable()));
    report.rebuild_totals();
    report.elapsed_ms = start.elapsed().as_millis();
    report
}

/// Convenience for headless use: runs a scan on a worker thread and waits.
pub fn run_blocking(opts: &ScanOptions) -> ScanReport {
    let (tx, rx) = crossbeam_channel::unbounded();
    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let opts = opts.clone();
        thread::spawn(move || run(&opts, tx, stop))
    };
    let _ = rx; // progress is not consumed here; the channel is unbounded
    worker.join().unwrap_or_else(|_| ScanReport {
        errors: vec!["scan thread panicked".into()],
        ..ScanReport::default()
    })
}
