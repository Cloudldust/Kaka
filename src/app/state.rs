//! Application state: current workspace, navigation, selection and counts.

use crate::db::{self, Db};
use crate::db::photos::StatusCounts;
use crate::model::*;
use std::collections::HashSet;

/// A single undoable status change (PRD 7.2 撤销重做). Only single Q/E/U key
/// operations enter the undo stack; batch operations and crash-recovery resets
/// do not.
#[derive(Debug, Clone, Copy)]
pub struct HistoryEntry {
    pub photo_id: i64,
    pub old_status: Status,
    pub new_status: Status,
}

/// The currently-loaded workspace (a folder prefix + filtered/sorted photos).
#[derive(Debug, Clone)]
pub struct Workspace {
    pub folder_path: String,
    pub sort: SortOrder,
    pub items: Vec<PhotoListItem>,
    pub current_index: usize,
    pub selection: HashSet<i64>,
    /// The anchor index for Shift+click range selection (PRD 7.9.1).
    pub selection_anchor: Option<usize>,
    /// Simple filename search filter (empty = no filter).
    pub search: String,
    pub counts: StatusCounts,
}

impl Workspace {
    pub fn empty() -> Self {
        Workspace {
            folder_path: String::new(),
            sort: SortOrder::CaptureTimeAsc,
            items: Vec::new(),
            current_index: 0,
            selection: HashSet::new(),
            selection_anchor: None,
            search: String::new(),
            counts: StatusCounts::default(),
        }
    }

    /// Number of photos under the current filter and sort.
    pub fn total(&self) -> usize {
        self.items.len()
    }

    pub fn current(&self) -> Option<&PhotoListItem> {
        self.items.get(self.current_index)
    }

    pub fn selected_count(&self) -> usize {
        self.selection.len()
    }
}

/// Current import progress, including a short phase label so the user can tell
/// what stage the import is in (e.g. 检查 / 拷贝).
#[derive(Debug, Clone, Default)]
pub struct ImportProgress {
    pub phase: String,
    pub done: usize,
    pub total: usize,
    pub filename: String,
}

/// Aggregated application state.
pub struct AppState {
    pub db: Db,
    pub config: AppConfig,
    pub ws: Workspace,
    pub folder_loaded: bool,

    // UI display preferences (persisted per workspace later).
    pub right_panel_visible: bool,
    pub right_panel_width: f32,
    pub thumb_strip_height: f32,

    // Modal / state flags.
    pub show_import: bool,
    pub show_settings: bool,
    pub show_delete_box: bool,
    pub show_crash_recovery: bool,
    pub crash_state: Option<WorkspaceState>,

    // Import progress (background job bridge).
    pub import_running: bool,
    pub import_progress: ImportProgress,
    pub import_result: Option<Result<ImportResult, String>>,

    // Undo/redo stack for single Q/E/U status changes (PRD 7.2). Cleared when
    // the workspace switches or the app closes; not persisted.
    pub undo_stack: Vec<HistoryEntry>,
    pub redo_stack: Vec<HistoryEntry>,
}

/// Outcome of either an add-mode or copy-mode import.
#[derive(Debug, Clone)]
pub enum ImportResult {
    Add(crate::app::import::ImportOutcome),
    Copy(crate::app::copy::CopyOutcome),
}

/// Which import mode the import dialog is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    Add,
    Copy,
}

impl ImportResult {
    /// Short summary for the completion toast.
    pub fn summary(&self) -> String {
        match self {
            ImportResult::Add(o) => format!(
                "已将 {} 张照片添加到图库，文件保留在原位",
                o.added
            ),
            ImportResult::Copy(o) => format!(
                "成功导入 {} 张（已存在跳过 {} 张，失败 {} 张）",
                o.copied, o.skipped_existing, o.failed
            ),
        }
    }
}

impl AppState {
    pub fn new(db: Db, config: AppConfig) -> Self {
        AppState {
            db,
            config,
            ws: Workspace::empty(),
            folder_loaded: false,
            right_panel_visible: true,
            right_panel_width: 260.0,
            thumb_strip_height: 120.0,
            show_import: false,
            show_settings: false,
            show_delete_box: false,
            show_crash_recovery: false,
            crash_state: None,
            import_running: false,
            import_progress: ImportProgress::default(),
            import_result: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Load a workspace rooted at `folder` with the given sort order.
    pub fn open_workspace(&mut self, folder: &str, sort: SortOrder) -> anyhow::Result<()> {
        if folder.is_empty() || !std::path::Path::new(folder).is_dir() {
            // Path no longer valid: fall back to empty state (UI spec 6.2).
            self.ws = Workspace::empty();
            self.folder_loaded = false;
            return Ok(());
        }
        // A real folder switch clears the undo/redo stacks (PRD 7.2).
        let is_switch = self.ws.folder_path != folder;
        db::folders::touch_open(&self.db, folder)?;
        let items = db::photos::list_items_in_folder(&self.db, folder, sort)?;
        let counts = db::photos::status_counts(&self.db, folder)?;
        self.ws = Workspace {
            folder_path: folder.to_string(),
            sort,
            items,
            current_index: 0,
            selection: HashSet::new(),
            selection_anchor: None,
            search: String::new(),
            counts,
        };
        if is_switch {
            self.undo_stack.clear();
            self.redo_stack.clear();
        }
        self.folder_loaded = true;
        Ok(())
    }

    /// Reload the current workspace preserving selection/sort/search (used on
    /// sort/filter changes within the same workspace).
    pub fn reload_current(&mut self) -> anyhow::Result<()> {
        if self.ws.folder_path.is_empty() {
            return Ok(());
        }
        let folder = self.ws.folder_path.clone();
        let sort = self.ws.sort;
        let current_id = self.ws.current().map(|p| p.id);
        let search = self.ws.search.clone();
        let selection = self.ws.selection.clone();
        let anchor = self.ws.selection_anchor;
        self.open_workspace(&folder, sort)?;
        self.ws.search = search;
        self.ws.selection = selection;
        self.ws.selection_anchor = anchor;
        // Re-locate the current photo by id (or its nearest predecessor).
        if let Some(id) = current_id {
            if let Some(pos) = self.ws.items.iter().position(|p| p.id == id) {
                self.ws.current_index = pos;
            } else if !self.ws.items.is_empty() {
                self.ws.current_index = 0;
            }
        }
        Ok(())
    }

    /// Move the current index by `delta`, clamped to the list range.
    /// Returns whether the move was blocked at a boundary.
    pub fn step(&mut self, delta: i64) -> bool {
        if self.ws.items.is_empty() {
            return false;
        }
        let len = self.ws.items.len() as i64;
        let next = self.ws.current_index as i64 + delta;
        if next < 0 {
            return true; // at first
        }
        if next >= len {
            return true; // at last
        }
        self.ws.current_index = next as usize;
        false
    }

    /// Jump to a specific index (Home/End/number jump).
    pub fn jump_to(&mut self, index: usize) {
        if self.ws.items.is_empty() {
            return;
        }
        let len = self.ws.items.len();
        self.ws.current_index = index.min(len - 1);
    }

    /// Apply a status change to a photo and update the in-memory item.
    /// Returns true if the status actually changed.
    pub fn set_status(&mut self, photo_id: i64, status: Status) -> anyhow::Result<bool> {
        let changed = if let Some(p) = self.ws.items.iter_mut().find(|p| p.id == photo_id) {
            if p.status != status {
                p.status = status;
                true
            } else {
                false
            }
        } else {
            false
        };
        if changed {
            db::photos::set_status(&self.db, photo_id, status)?;
            self.refresh_counts()?;
        }
        Ok(changed)
    }

    /// Apply a status to the currently displayed photo (Q/E/U). When
    /// `record_history` is true, a single-key operation is recorded on the undo
    /// stack (PRD 7.2). Returns true if the status actually changed.
    pub fn set_status_current(&mut self, status: Status, record_history: bool) -> anyhow::Result<bool> {
        if let Some(p) = self.ws.current().cloned() {
            if p.status == status {
                return Ok(false);
            }
            if record_history {
                self.push_undo(HistoryEntry {
                    photo_id: p.id,
                    old_status: p.status,
                    new_status: status,
                });
            }
            self.set_status(p.id, status)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Apply a status to every selected photo. Batch operations do NOT enter the
    /// undo stack (PRD 7.2). Returns how many photos were actually changed.
    pub fn set_status_selected(&mut self, status: Status) -> anyhow::Result<usize> {
        let ids: Vec<i64> = self.ws.selection.iter().copied().collect();
        if ids.is_empty() {
            return Ok(0);
        }
        let n = db::photos::set_status_batch(&self.db, &ids, status)?;
        for id in &ids {
            if let Some(p) = self.ws.items.iter_mut().find(|p| p.id == *id) {
                p.status = status;
            }
        }
        self.refresh_counts()?;
        Ok(n)
    }

    /// Handle a thumbnail-strip click: plain click selects one photo and makes it
    /// current; Ctrl+click toggles it into/out of the selection; Shift+click
    /// range-selects from the anchor (PRD 7.9.1).
    pub fn select_click(&mut self, idx: usize, ctrl: bool, shift: bool) {
        let Some(item) = self.ws.items.get(idx).cloned() else {
            return;
        };
        if shift {
            let anchor = self.ws.selection_anchor.unwrap_or(self.ws.current_index);
            let (lo, hi) = (anchor.min(idx), anchor.max(idx));
            for i in lo..=hi {
                if let Some(it) = self.ws.items.get(i) {
                    self.ws.selection.insert(it.id);
                }
            }
            self.ws.selection_anchor = Some(anchor);
        } else if ctrl {
            if self.ws.selection.contains(&item.id) {
                self.ws.selection.remove(&item.id);
            } else {
                self.ws.selection.insert(item.id);
            }
            self.ws.selection_anchor = Some(idx);
        } else {
            self.ws.selection.clear();
            self.ws.selection.insert(item.id);
            self.ws.selection_anchor = Some(idx);
            self.ws.current_index = idx;
        }
    }

    /// Select every photo in the current (filtered) view, or clear it.
    pub fn select_all(&mut self, select: bool) {
        if select {
            for p in &self.ws.items {
                self.ws.selection.insert(p.id);
            }
            self.ws.selection_anchor = None;
        } else {
            self.ws.selection.clear();
            self.ws.selection_anchor = None;
        }
    }

    /// Clear the current selection (Esc when nothing else to handle, PRD 7.2).
    pub fn clear_selection(&mut self) -> bool {
        if !self.ws.selection.is_empty() {
            self.ws.selection.clear();
            self.ws.selection_anchor = None;
            true
        } else {
            false
        }
    }

    fn push_undo(&mut self, entry: HistoryEntry) {
        if self.undo_stack.len() >= 100 {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(entry);
        self.redo_stack.clear();
    }

    /// Undo the last single status change. Returns true if any step was undone.
    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo_stack.pop() else {
            return false;
        };
        let _ = self.set_status(entry.photo_id, entry.old_status);
        self.redo_stack.push(entry);
        true
    }

    /// Redo the last undone status change. Returns true if any step was re-applied.
    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.redo_stack.pop() else {
            return false;
        };
        let _ = self.set_status(entry.photo_id, entry.new_status);
        self.undo_stack.push(entry);
        true
    }

    /// Recompute the status counts for the current folder.
    pub fn refresh_counts(&mut self) -> anyhow::Result<()> {
        if self.ws.folder_path.is_empty() {
            return Ok(());
        }
        self.ws.counts = db::photos::status_counts(&self.db, &self.ws.folder_path)?;
        Ok(())
    }

    /// Close the workspace and persist nothing (called on folder switch).
    pub fn close_workspace(&mut self) {
        self.ws = Workspace::empty();
        self.folder_loaded = false;
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}
