use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::model::ScanReport;
use crate::scan::{ScanMsg, ScanOptions, run};
use crate::select::{KeepStrategy, keep_index, mark_victims};
use crate::trashbin::{self, DeleteFile, DeleteProgress};
use crate::util::format_size;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Scanning,
    Browse,
    Deleting,
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Groups,
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteKind {
    Trash,
    Purge,
}

#[derive(Debug, Clone)]
pub struct Pending {
    pub kind: DeleteKind,
    pub files: Vec<DeleteFile>,
    pub bytes: u64,
}

enum DeleteMsg {
    Progress(DeleteProgress),
    Finished(trashbin::DeleteOutcome),
}

pub struct App {
    pub opts: ScanOptions,
    pub keep: KeepStrategy,
    pub phase: Phase,
    pub report: Option<ScanReport>,
    pub progress: Option<u8>,
    pub progress_label: String,
    /// Parallel to `report.groups[*].files`.
    pub marked: Vec<Vec<bool>>,
    pub group_idx: usize,
    pub file_idx: usize,
    pub focus: Focus,
    pub filter: String,
    pub input: Option<String>,
    pub confirm: Option<Pending>,
    pub help: bool,
    pub status: String,
    pub quit: bool,
    pub groups_state: ListState,
    pub files_state: ListState,
    pub delete_kind: Option<DeleteKind>,
    pub delete_completed: usize,
    pub delete_total: usize,
    pub delete_freed: u64,
    pub delete_bytes_total: u64,
    pub delete_cancelling: bool,
    pub spin: usize,
    rx: Receiver<ScanMsg>,
    stop: Arc<AtomicBool>,
    delete_rx: Option<Receiver<DeleteMsg>>,
    delete_stop: Arc<AtomicBool>,
}

impl App {
    pub fn new(opts: ScanOptions, keep: KeepStrategy) -> Self {
        let (tx, rx) = unbounded();
        let stop = Arc::new(AtomicBool::new(false));
        spawn_scan(&opts, tx, stop.clone());
        Self {
            opts,
            keep,
            phase: Phase::Scanning,
            report: None,
            progress: Some(0),
            progress_label: "collecting files…".to_string(),
            marked: Vec::new(),
            group_idx: 0,
            file_idx: 0,
            focus: Focus::Groups,
            filter: String::new(),
            input: None,
            confirm: None,
            help: false,
            status: String::new(),
            quit: false,
            groups_state: ListState::default(),
            files_state: ListState::default(),
            delete_kind: None,
            delete_completed: 0,
            delete_total: 0,
            delete_freed: 0,
            delete_bytes_total: 0,
            delete_cancelling: false,
            spin: 0,
            rx,
            stop,
            delete_rx: None,
            delete_stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn tick(&mut self) {
        if self.phase == Phase::Deleting {
            self.spin = self.spin.wrapping_add(1);
        }
        self.poll_delete();
        loop {
            match self.rx.try_recv() {
                Ok(ScanMsg::Progress { pct, label }) => {
                    self.progress = pct;
                    self.progress_label = label;
                }
                Ok(ScanMsg::Done(report)) => {
                    let report = *report;
                    self.progress = Some(100);
                    self.marked = report
                        .groups
                        .iter()
                        .map(|group| mark_victims(group, self.keep))
                        .collect();
                    self.phase = if report.groups.is_empty() {
                        Phase::Empty
                    } else {
                        Phase::Browse
                    };
                    if let Some(err) = report.errors.first() {
                        self.status = err.clone();
                    } else if report.groups.is_empty() {
                        self.status = "no duplicates found".to_string();
                    } else {
                        let victims = self.marked.iter().flatten().filter(|m| **m).count();
                        self.status = format!(
                            "Smart Selection pre-marked {victims} files · review, then press d"
                        );
                    }
                    self.report = Some(report);
                    self.clamp();
                }
                Ok(ScanMsg::Failed(err)) => {
                    self.status = err;
                    self.phase = Phase::Empty;
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    // ---- navigation -------------------------------------------------------

    pub fn visible_groups(&self) -> Vec<usize> {
        let Some(report) = &self.report else {
            return Vec::new();
        };
        if self.filter.is_empty() {
            return (0..report.groups.len()).collect();
        }
        let needle = self.filter.to_lowercase();
        report
            .groups
            .iter()
            .enumerate()
            .filter(|(_, group)| {
                group
                    .files
                    .iter()
                    .any(|f| f.path.to_string_lossy().to_lowercase().contains(&needle))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn current_group_index(&self) -> Option<usize> {
        self.visible_groups().get(self.group_idx).copied()
    }

    pub fn current_group(&self) -> Option<&crate::model::DupeGroup> {
        let gi = self.current_group_index()?;
        self.report.as_ref()?.groups.get(gi)
    }

    pub fn current_file(&self) -> Option<&crate::model::DupeFile> {
        let group = self.current_group()?;
        group.files.get(self.file_idx)
    }

    pub fn file_len(&self, group_index: usize) -> usize {
        self.report
            .as_ref()
            .and_then(|r| r.groups.get(group_index))
            .map(|g| g.files.len())
            .unwrap_or(0)
    }

    pub fn clamp(&mut self) {
        let visible = self.visible_groups();
        if visible.is_empty() {
            self.group_idx = 0;
            self.file_idx = 0;
        } else {
            self.group_idx = self.group_idx.min(visible.len() - 1);
            let len = self.file_len(visible[self.group_idx]);
            self.file_idx = self.file_idx.min(len.saturating_sub(1));
        }
        self.groups_state.select(Some(self.group_idx));
        self.files_state
            .select((!visible.is_empty()).then_some(self.file_idx));
    }

    fn move_by(&mut self, delta: isize) {
        match self.focus {
            Focus::Groups => {
                let len = self.visible_groups().len();
                if len == 0 {
                    return;
                }
                let max = len as isize - 1;
                let next = (self.group_idx as isize + delta).clamp(0, max) as usize;
                if next != self.group_idx {
                    self.group_idx = next;
                    self.file_idx = 0;
                }
            }
            Focus::Files => {
                let Some(gi) = self.current_group_index() else {
                    return;
                };
                let len = self.file_len(gi);
                if len == 0 {
                    return;
                }
                let max = len as isize - 1;
                self.file_idx = (self.file_idx as isize + delta).clamp(0, max) as usize;
            }
        }
        self.clamp();
    }

    fn jump(&mut self, to_end: bool) {
        match self.focus {
            Focus::Groups => {
                let len = self.visible_groups().len();
                if len > 0 {
                    self.group_idx = if to_end { len - 1 } else { 0 };
                    self.file_idx = 0;
                }
            }
            Focus::Files => {
                let Some(gi) = self.current_group_index() else {
                    return;
                };
                let len = self.file_len(gi);
                if len > 0 {
                    self.file_idx = if to_end { len - 1 } else { 0 };
                }
            }
        }
        self.clamp();
    }

    // ---- selection --------------------------------------------------------

    pub fn reapply_smart_select(&mut self) {
        let keep = self.keep;
        if let Some(report) = &self.report {
            self.marked = report
                .groups
                .iter()
                .map(|group| mark_victims(group, keep))
                .collect();
        }
    }

    pub fn marked_count(&self) -> usize {
        self.marked.iter().flatten().filter(|m| **m).count()
    }

    pub fn marked_bytes(&self) -> u64 {
        let Some(report) = &self.report else {
            return 0;
        };
        let mut total = 0;
        for (gi, group) in report.groups.iter().enumerate() {
            let Some(row) = self.marked.get(gi) else {
                continue;
            };
            for (fi, file) in group.files.iter().enumerate() {
                if row.get(fi).copied().unwrap_or(false) {
                    total += file.size;
                }
            }
        }
        total
    }

    /// Index of the copy Smart Selection would keep in the focused group.
    pub fn keeper_index(&self, group_index: usize) -> Option<usize> {
        self.report
            .as_ref()?
            .groups
            .get(group_index)
            .map(|g| keep_index(g, self.keep))
    }

    fn toggle_mark(&mut self) {
        let Some(gi) = self.current_group_index() else {
            return;
        };
        self.clamp();
        if let Some(slot) = self
            .marked
            .get_mut(gi)
            .and_then(|row| row.get_mut(self.file_idx))
        {
            *slot = !*slot;
        }
    }

    fn mark_group(&mut self) {
        let Some(gi) = self.current_group_index() else {
            return;
        };
        let Some(group) = self.report.as_ref().and_then(|r| r.groups.get(gi)) else {
            return;
        };
        let marks = mark_victims(group, self.keep);
        if let Some(slot) = self.marked.get_mut(gi) {
            *slot = marks;
        }
    }

    fn clear_marks(&mut self) {
        for row in &mut self.marked {
            row.fill(false);
        }
    }

    // ---- deletion ---------------------------------------------------------

    fn request_delete(&mut self, kind: DeleteKind) {
        let Some(report) = &self.report else {
            return;
        };
        let mut files = Vec::new();
        let mut bytes = 0;
        let mut risky = None;
        for (gi, group) in report.groups.iter().enumerate() {
            let Some(row) = self.marked.get(gi) else {
                continue;
            };
            let marked_here = row.iter().filter(|m| **m).count();
            if marked_here > 0 && marked_here == group.files.len() {
                risky = Some(group.files[0].path.clone());
                break;
            }
            for (fi, file) in group.files.iter().enumerate() {
                if row.get(fi).copied().unwrap_or(false) {
                    files.push(DeleteFile {
                        path: file.path.clone(),
                        size: file.size,
                    });
                    bytes += file.size;
                }
            }
        }
        if let Some(path) = risky {
            self.status = format!("refusing to remove every copy of {}", path.display());
            return;
        }
        if files.is_empty() {
            self.status = "nothing marked — press a for Smart Selection".to_string();
            return;
        }
        let verb = match kind {
            DeleteKind::Trash => "Move to Trash",
            DeleteKind::Purge => "Permanently delete",
        };
        self.status = format!(
            "{verb} {} files ({})?  y = confirm · n = cancel",
            files.len(),
            format_size(bytes)
        );
        self.confirm = Some(Pending { kind, files, bytes });
    }

    fn confirm_delete(&mut self) {
        let Some(pending) = self.confirm.take() else {
            return;
        };
        let kind = pending.kind;
        let total = pending.files.len();
        let bytes = pending.bytes;
        self.phase = Phase::Deleting;
        self.delete_kind = Some(kind);
        self.delete_completed = 0;
        self.delete_total = total;
        self.delete_freed = 0;
        self.delete_bytes_total = bytes;
        self.delete_cancelling = false;
        self.help = false;
        self.spin = 0;

        let (tx, rx) = unbounded();
        self.delete_rx = Some(rx);
        self.delete_stop = Arc::new(AtomicBool::new(false));
        let stop = self.delete_stop.clone();
        let roots = self.opts.roots.clone();
        let progress_tx = tx.clone();
        thread::spawn(move || {
            let outcome = match kind {
                DeleteKind::Trash => trashbin::trash_files(&pending.files, &stop, |progress| {
                    let _ = progress_tx.send(DeleteMsg::Progress(progress));
                }),
                DeleteKind::Purge => {
                    trashbin::purge_files(&pending.files, &roots, &stop, |progress| {
                        let _ = progress_tx.send(DeleteMsg::Progress(progress));
                    })
                }
            };
            let _ = tx.send(DeleteMsg::Finished(outcome));
        });
    }

    fn cancel_delete(&mut self) {
        self.delete_stop.store(true, Ordering::Release);
        self.delete_cancelling = true;
    }

    fn poll_delete(&mut self) {
        let Some(rx) = self.delete_rx.take() else {
            return;
        };
        let mut put_back = true;
        loop {
            match rx.try_recv() {
                Ok(DeleteMsg::Progress(progress)) => {
                    self.delete_completed = progress.completed;
                    self.delete_total = progress.total;
                    self.delete_freed = progress.freed;
                }
                Ok(DeleteMsg::Finished(outcome)) => {
                    self.finish_delete(outcome);
                    put_back = false;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.status = "delete stopped unexpectedly".to_string();
                    self.leave_delete();
                    put_back = false;
                    break;
                }
            }
        }
        if put_back {
            self.delete_rx = Some(rx);
        }
    }

    fn finish_delete(&mut self, outcome: trashbin::DeleteOutcome) {
        let anchor_hash = self.current_group().map(|group| group.hash.clone());
        let anchor_file = self.current_file().map(|file| file.path.clone());
        let removed: HashSet<PathBuf> = outcome.deleted.iter().cloned().collect();
        if let Some(report) = self.report.as_mut() {
            excise_deleted(report, &mut self.marked, &removed);
        }
        if let Some(hash) = anchor_hash
            && let Some(report) = &self.report
            && let Some(group_at) = report.groups.iter().position(|group| group.hash == hash)
        {
            let visible = self.visible_groups();
            if let Some(pos) = visible.iter().position(|&index| index == group_at) {
                self.group_idx = pos;
            }
        }
        if let Some(path) = anchor_file
            && let Some(group) = self.current_group()
            && let Some(file_at) = group.files.iter().position(|file| file.path == path)
        {
            self.file_idx = file_at;
        }
        self.clamp();
        let kind = self.delete_kind.unwrap_or(DeleteKind::Trash);
        self.status = finish_status(kind, &outcome);
        self.leave_delete();
    }

    fn leave_delete(&mut self) {
        self.delete_rx = None;
        self.delete_kind = None;
        self.delete_cancelling = false;
        self.phase = if self.report.as_ref().is_some_and(|r| !r.groups.is_empty()) {
            Phase::Browse
        } else {
            Phase::Empty
        };
    }

    // ---- actions ----------------------------------------------------------

    pub fn rescan(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let (tx, rx) = unbounded();
        self.rx = rx;
        self.stop = Arc::new(AtomicBool::new(false));
        spawn_scan(&self.opts, tx, self.stop.clone());
        self.phase = Phase::Scanning;
        self.report = None;
        self.marked.clear();
        self.group_idx = 0;
        self.file_idx = 0;
        self.filter.clear();
        self.input = None;
        self.confirm = None;
        self.progress = Some(0);
        self.progress_label = "collecting files…".to_string();
        self.status = "rescanning…".to_string();
    }

    fn reveal(&mut self) {
        let Some(file) = self.current_file() else {
            return;
        };
        let path = file.path.clone();
        match std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .spawn()
        {
            Ok(_) => self.status = format!("revealed {}", path.display()),
            Err(err) => self.status = format!("could not reveal: {err}"),
        }
    }

    fn request_quit(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.quit = true;
    }

    // ---- input ------------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.phase == Phase::Deleting {
                self.cancel_delete();
            }
            self.request_quit();
            return;
        }
        if self.phase == Phase::Deleting {
            if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                self.cancel_delete();
            }
            return;
        }
        if self.input.is_some() {
            self.on_filter_key(key);
            return;
        }
        if self.confirm.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.confirm_delete(),
                _ => {
                    self.confirm = None;
                    self.status = "cancelled".to_string();
                }
            }
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        if self.phase == Phase::Scanning {
            if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                self.request_quit();
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.request_quit(),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::Char('g') | KeyCode::Home => self.jump(false),
            KeyCode::Char('G') | KeyCode::End => self.jump(true),
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Groups => Focus::Files,
                    Focus::Files => Focus::Groups,
                }
            }
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Groups,
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => self.focus = Focus::Files,
            KeyCode::Char(' ') => match self.focus {
                Focus::Groups => self.mark_group(),
                Focus::Files => self.toggle_mark(),
            },
            KeyCode::Char('a') => {
                self.reapply_smart_select();
                self.status = format!(
                    "Smart Selection marked {} files ({})",
                    self.marked_count(),
                    format_size(self.marked_bytes())
                );
            }
            KeyCode::Char('A') => {
                self.clear_marks();
                self.status = "cleared all marks".to_string();
            }
            KeyCode::Char('s') => {
                self.keep = self.keep.next();
                self.reapply_smart_select();
                self.status = format!(
                    "keep strategy: {} · re-marked {} files",
                    self.keep.label(),
                    self.marked_count()
                );
            }
            KeyCode::Char('d') => self.request_delete(DeleteKind::Trash),
            KeyCode::Char('D') => self.request_delete(DeleteKind::Purge),
            KeyCode::Char('o') => self.reveal(),
            KeyCode::Char('r') => self.rescan(),
            KeyCode::Char('/') => {
                self.input = Some(self.filter.clone());
                self.focus = Focus::Groups;
            }
            _ => {}
        }
    }

    fn on_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.input = None;
                self.filter.clear();
                self.group_idx = 0;
                self.clamp();
            }
            KeyCode::Enter => {
                self.filter = self.input.take().unwrap_or_default();
                self.group_idx = 0;
                self.clamp();
            }
            KeyCode::Backspace => {
                if let Some(text) = &mut self.input {
                    text.pop();
                    self.filter = text.clone();
                    self.group_idx = 0;
                    self.clamp();
                }
            }
            KeyCode::Char(c) => {
                if let Some(text) = &mut self.input {
                    text.push(c);
                    self.filter = text.clone();
                    self.group_idx = 0;
                    self.clamp();
                }
            }
            _ => {}
        }
    }
}

/// Drop removed paths out of the report and keep survivor marks aligned.
fn excise_deleted(
    report: &mut ScanReport,
    marked: &mut Vec<Vec<bool>>,
    removed: &HashSet<PathBuf>,
) {
    let groups = std::mem::take(&mut report.groups);
    let mut marks = std::mem::take(marked);
    if marks.len() < groups.len() {
        marks.resize_with(groups.len(), Vec::new);
    }
    let mut new_groups = Vec::new();
    let mut new_marks = Vec::new();
    for (mut group, row) in groups.into_iter().zip(marks) {
        let mut files = Vec::new();
        let mut next_marks = Vec::new();
        for (idx, file) in std::mem::take(&mut group.files).into_iter().enumerate() {
            if !removed.contains(&file.path) {
                next_marks.push(row.get(idx).copied().unwrap_or(false));
                files.push(file);
            }
        }
        if files.len() >= 2 {
            group.files = files;
            new_groups.push(group);
            new_marks.push(next_marks);
        }
    }
    report.groups = new_groups;
    *marked = new_marks;
    report.rebuild_totals();
}

fn finish_status(kind: DeleteKind, outcome: &trashbin::DeleteOutcome) -> String {
    let action = match kind {
        DeleteKind::Trash => "moved to Trash",
        DeleteKind::Purge => "deleted",
    };
    let mut message = if outcome.deleted.is_empty() {
        format!("nothing {action}")
    } else {
        format!(
            "{} files {action} · freed {}",
            outcome.deleted.len(),
            format_size(outcome.freed)
        )
    };
    if outcome.cancelled {
        message = format!("cancelled · {message}");
    }
    if let Some((_, err)) = outcome.failed.first() {
        message = format!("{message} · {} failed ({err})", outcome.failed.len());
    }
    message
}

#[cfg(test)]
pub(crate) fn blank_for_test() -> App {
    let (_tx, rx) = unbounded();
    App {
        opts: ScanOptions {
            roots: Vec::new(),
            min_size: 1,
            extensions: Vec::new(),
            extra_excludes: Vec::new(),
            use_cache: false,
        },
        keep: KeepStrategy::Oldest,
        phase: Phase::Browse,
        report: None,
        progress: None,
        progress_label: String::new(),
        marked: Vec::new(),
        group_idx: 0,
        file_idx: 0,
        focus: Focus::Groups,
        filter: String::new(),
        input: None,
        confirm: None,
        help: false,
        status: String::new(),
        quit: false,
        groups_state: ListState::default(),
        files_state: ListState::default(),
        delete_kind: None,
        delete_completed: 0,
        delete_total: 0,
        delete_freed: 0,
        delete_bytes_total: 0,
        delete_cancelling: false,
        spin: 0,
        rx,
        stop: Arc::new(AtomicBool::new(false)),
        delete_rx: None,
        delete_stop: Arc::new(AtomicBool::new(false)),
    }
}

fn spawn_scan(opts: &ScanOptions, tx: Sender<ScanMsg>, stop: Arc<AtomicBool>) {
    let opts = opts.clone();
    thread::spawn(move || {
        let report = run(&opts, tx.clone(), stop);
        let _ = tx.send(ScanMsg::Done(Box::new(report)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DupeFile, DupeGroup, ScanReport};
    use crate::trashbin;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn file(path: &str) -> DupeFile {
        DupeFile {
            path: PathBuf::from(path),
            size: 10,
            modified: 1,
        }
    }

    #[test]
    fn excise_drops_resolved_groups_and_keeps_remaining_marks() {
        let mut report = ScanReport {
            groups: vec![
                DupeGroup {
                    size: 10,
                    hash: "a".into(),
                    files: vec![file("/a"), file("/a-copy")],
                },
                DupeGroup {
                    size: 10,
                    hash: "b".into(),
                    files: vec![file("/b"), file("/b-copy"), file("/b-2")],
                },
            ],
            ..ScanReport::default()
        };
        let mut marked = vec![vec![false, true], vec![false, true, true]];
        let removed = HashSet::from([PathBuf::from("/a-copy"), PathBuf::from("/b-copy")]);

        excise_deleted(&mut report, &mut marked, &removed);

        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].hash, "b");
        assert_eq!(
            report.groups[0]
                .files
                .iter()
                .map(|f| f.path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["/b".to_string(), "/b-2".to_string()]
        );
        assert_eq!(marked, vec![vec![false, true]]);
        assert_eq!(report.reclaimable, 10);
    }

    #[test]
    fn cancel_status_names_what_already_finished() {
        let outcome = trashbin::DeleteOutcome {
            deleted: vec![PathBuf::from("/a")],
            failed: vec![],
            freed: 2048,
            cancelled: true,
        };
        let message = finish_status(DeleteKind::Trash, &outcome);
        assert!(message.starts_with("cancelled · "));
        assert!(message.contains("1 files moved to Trash"));
        assert!(message.contains("2.0 KB"));
    }
}
