//! The eframe application shell: startup, event loop, layout orchestration.

use super::super::{import, state::AppState};
use super::texture::TextureCache;
use super::{dialogs, theme, view};
use crate::app::card::{CardDetector, CardEvent};
use crate::app::memcache::MemLru;
use crate::app::thumbs::ThumbWorker;
use crate::app::zoom::{ZoomMsg, ZoomWorker};
use crate::config;
use crate::db::{self, Db};
use crate::model::*;
use eframe::egui;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

/// How many of the first imported photos get background thumbnail generation
/// requested during the import (concurrent, so it never blocks the import loop).
const ADD_THUMB_PREWARM: usize = 16;

/// Memory cap for the Z-key full-resolution RAW texture LRU (PRD 7.4 / 9.5:
/// 缓存在内存中，LRU 策略，总上限 2GB).
const ZOOM_TEX_CAP_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Small cleanup budget: max files removed per incremental cache clean (PRD 9.4).
const CACHE_CLEAN_MAX_FILES: usize = 100;

/// Idle interval that also triggers an incremental cache clean (PRD 9.4: 60s).
const CACHE_CLEAN_IDLE: std::time::Duration = std::time::Duration::from_secs(60);

/// Messages sent from the background import thread back to the UI.
pub enum ImportMsg {
    Progress {
        phase: String,
        done: usize,
        total: usize,
        filename: String,
    },
    /// A newly-imported photo that should have its thumbnail generated in the
    /// background (the UI forwards these to the ThumbWorker with priority).
    ThumbJob {
        photo_id: i64,
        hash: String,
        path: String,
    },
    Done(Box<Result<crate::app::state::ImportResult, String>>),
}

/// Toast severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    pub created: std::time::Instant,
    pub ttl_secs: f64,
}

/// Startup diagnostics gathered before the UI loop.
#[derive(Default)]
pub struct StartupInfo {
    pub corruption_detected: bool,
    pub db_repaired: bool,
    pub first_run: bool,
}

pub struct KakaApp {
    pub state: AppState,
    pub textures: TextureCache,
    pub thumbs: ThumbWorker,

    pub toasts: Vec<Toast>,

    // Import (add-mode) background job state.
    pub import_rx: Option<Receiver<ImportMsg>>,
    pub import_cancel: Arc<AtomicBool>,
    pub import_path: String,
    pub import_recursive: bool,
    pub import_dedup: bool,
    pub import_mode: crate::app::state::ImportMode,
    pub import_target: String,
    pub import_org: crate::app::copy::OrgMode,
    /// Export dialog defaults (PRD 12).
    pub export_target: String,
    pub export_org: crate::app::copy::OrgMode,
    /// Detected Lightroom Classic exe path (optional feature, PRD 13).
    pub lr_path: Option<std::path::PathBuf>,
    /// 清空存储卡 (PRD 6.7): move successfully-copied source files on the
    /// removable card to the recycle bin after a fully-successful import.
    pub import_clear_card: bool,

    // Zoom (Z-key) view state (PRD 7.4). The pan anchor is stored as the image
    // point (fractions 0..1) shown at the viewport center, so it survives the
    // preview -> RAW texture swap unchanged (无缝替换).
    pub zoom_active: bool,
    pub zoom_center: (f32, f32),
    pub zoom_photo_id: Option<i64>,
    /// Full-resolution RAW decoder for the 100% view (PRD 7.4 视口解码).
    pub zoom_worker: ZoomWorker,
    /// Decoded full-res textures, LRU-capped at 2 GB (PRD 7.4 内存缓存).
    pub zoom_tex: MemLru<(i64, String), egui::TextureHandle>,
    /// Full-resolution dimensions known so far (EXIF hint / decode result).
    pub zoom_dims: std::collections::HashMap<i64, (u32, u32)>,
    /// Per-photo remembered pan anchors for the session (PRD 7.4.1).
    pub zoom_anchors: std::collections::HashMap<i64, (f32, f32)>,

    // Advanced-filter dialog draft (PRD 7.8), applied only on "应用".
    pub filter_draft: crate::model::Filter,

    // Settings dialog working draft (only applied on "保存").
    pub settings_draft: crate::model::AppConfig,

    // SD card hot-plug detector.
    pub card: CardDetector,

    // Crash recovery pending state.
    pub pending_crash: Option<WorkspaceState>,

    // Interrupted-import resume pending state (PRD 6.7.1).
    pub show_resume: bool,
    pub pending_resume: Option<crate::app::session::ImportSession>,

    // Autosave.
    pub last_autosave: std::time::Instant,
    pub needs_save: bool,

    // Thumbnail strip auto-centering (last focused photo id we centered).
    pub last_centered_id: Option<i64>,

    // Last workspace folder we enqueued missing thumbnails for.
    pub last_ws_folder: String,

    pub startup: StartupInfo,

    // Confirm dialog (generic).
    pub confirm: Option<ConfirmDialog>,

    // Disk-cache cleaner (PRD 9.4): small incremental cleans run in a
    // background thread, triggered by browsing 50 photos or idling 60s.
    pub cache_clean_rx: Option<Receiver<anyhow::Result<crate::io::cache_clean::CleanStats>>>,
    pub cache_clean_running: bool,
    /// True when the running clean was requested from settings (reports a toast).
    pub cache_clean_full: bool,
    pub cache_clean_progress: Arc<AtomicUsize>,
    pub photos_since_clean: usize,
    pub last_viewed_id: Option<i64>,
    pub last_clean_at: std::time::Instant,
}

pub struct ConfirmDialog {
    pub title: String,
    pub text: String,
    pub confirm_label: String,
    pub danger: bool,
    pub on_confirm: Box<dyn FnOnce(&mut KakaApp)>,
}

impl KakaApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        db: Db,
        cfg: AppConfig,
        startup: StartupInfo,
        was_crash: bool,
    ) -> Self {
        theme::setup_fonts(&cc.egui_ctx);
        theme::apply_style(&cc.egui_ctx);

        let settings_draft = cfg.clone();
        let state = AppState::new(db, cfg);

        let pending_crash = if was_crash {
            db::workspace::load(&state.db).ok().flatten()
        } else {
            None
        };

        let mut app = KakaApp {
            state,
            textures: TextureCache::new(),
            thumbs: ThumbWorker::new(),
            toasts: Vec::new(),
            import_rx: None,
            import_cancel: Arc::new(AtomicBool::new(false)),
            import_path: String::new(),
            import_recursive: true,
            import_dedup: true,
            import_mode: crate::app::state::ImportMode::Add,
            import_target: String::new(),
            import_org: crate::app::copy::OrgMode::Structure,
            export_target: String::new(),
            export_org: crate::app::copy::OrgMode::Structure,
            lr_path: None,
            import_clear_card: false,
            zoom_active: false,
            zoom_center: (0.5, 0.5),
            zoom_photo_id: None,
            zoom_worker: ZoomWorker::new(),
            zoom_tex: MemLru::new(ZOOM_TEX_CAP_BYTES),
            zoom_dims: std::collections::HashMap::new(),
            zoom_anchors: std::collections::HashMap::new(),
            filter_draft: crate::model::Filter::default(),
            settings_draft,
            card: crate::app::card::CardDetector::new(),
            pending_crash,
            show_resume: false,
            pending_resume: None,
            last_autosave: std::time::Instant::now(),
            needs_save: false,
            last_centered_id: None,
            last_ws_folder: String::new(),
            startup,
            confirm: None,
            cache_clean_rx: None,
            cache_clean_running: false,
            cache_clean_full: false,
            cache_clean_progress: Arc::new(AtomicUsize::new(0)),
            photos_since_clean: 0,
            last_viewed_id: None,
            last_clean_at: std::time::Instant::now(),
        };
        if app.startup.first_run {
            app.toast(
                ToastKind::Info,
                "欢迎使用咔咔！只做导入+筛选。点击「导入」开始添加照片。",
            );
        }
        app
    }
}

/// Launch the GUI. This owns init, and blocks until the window closes.
pub fn run() -> anyhow::Result<()> {
    let _ = env_logger::try_init();
    crate::paths::ensure_dirs()?;

    // 1. Config.
    let cfg = config::load();

    // 2. Database open + integrity + migration.
    let (db, startup) = init_database()?;

    // 3. Crash marker bookkeeping.
    let was_crash = db::workspace::crash_marker(&db)?;
    db::workspace::mark_crash(&db)?;

    // 4. Build the app.
    let icon = load_icon();
    let mut vb = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([1024.0, 640.0])
        .with_title("咔咔 · Kaka");
    if let Some(icon) = icon {
        vb = vb.with_icon(icon);
    }
    let native = eframe::NativeOptions {
        viewport: vb,
        ..Default::default()
    };

    eframe::run_native(
        "kaka",
        native,
        Box::new(move |cc| {
            let mut app = KakaApp::new(cc, db, cfg, startup, was_crash);
            // Resume prompt comes before crash recovery (PRD 6.1 startup order).
            if let Some(s) = crate::app::session::list_incomplete().into_iter().next() {
                app.pending_resume = Some(s);
                app.show_resume = true;
            }
            if app.pending_crash.is_some() {
                app.state.show_crash_recovery = true;
            } else if app.state.config.auto_open_last_workspace {
                if let Ok(Some(saved)) = db::workspace::load(&app.state.db) {
                    if let Some(folder) = saved.current_folder_path {
                        let sort = SortOrder::from_code(&saved.current_sort);
                        let _ = app.state.open_workspace(&folder, sort);
                        app.state.ws.current_index = saved.current_index.max(0) as usize;
                    }
                }
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("启动失败: {e}"))?;

    // 5. Clean exit: clear the crash marker via a fresh connection.
    if let Ok(d) = Db::open_default() {
        let _ = db::workspace::clear_crash(&d);
    }
    Ok(())
}

fn init_database() -> anyhow::Result<(Db, StartupInfo)> {
    let mut startup = StartupInfo::default();
    let mut db = Db::open_default()?;

    // Integrity check (PRD 10.6). On failure try to repair or reset.
    if !db.integrity_check()? {
        startup.corruption_detected = true;
        log::error!("数据库完整性检查失败，尝试修复");
        db::schema::repair_or_reset(&mut db)?;
        startup.db_repaired = true;
    }

    // Create schema + run migrations (PRD 10.5).
    db::schema::init(&mut db)?;
    db::schema::migrate(&mut db)?;

    // Detect first run (empty DB).
    if db::photos::status_counts(&db, "")?.total == 0 {
        startup.first_run = true;
    }
    Ok((db, startup))
}

/// Decode the embedded KAKA.ico into an egui window icon (taskbar / title bar).
/// The ICO bytes are baked into the binary at compile time so the window icon
/// matches the packaged exe regardless of the runtime working directory.
fn load_icon() -> Option<egui::viewport::IconData> {
    let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/KAKA.ico"));
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Some(egui::viewport::IconData {
        rgba: rgba.into_raw(),
        width: w,
        height: h,
    })
}

impl KakaApp {
    /// Push a toast notification.
    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        let ttl = match kind {
            ToastKind::Info | ToastKind::Success => 3.0,
            ToastKind::Warning => 8.0,
            ToastKind::Error => 12.0,
        };
        self.toasts.push(Toast {
            kind,
            text: text.into(),
            created: std::time::Instant::now(),
            ttl_secs: ttl,
        });
        if self.toasts.len() > 6 {
            self.toasts.remove(0);
        }
    }

    fn expire_toasts(&mut self) {
        let now = std::time::Instant::now();
        self.toasts.retain(|t| now.duration_since(t.created).as_secs_f64() < t.ttl_secs);
    }

    /// Poll the background import job and fold its messages into state.
    fn poll_import(&mut self) {
        let Some(rx) = self.import_rx.take() else {
            return;
        };
        let mut progress = None;
        let mut result: Option<Result<crate::app::state::ImportResult, String>> = None;
        let mut finish = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                ImportMsg::Progress {
                    phase,
                    done,
                    total,
                    filename,
                } => {
                    progress = Some(crate::app::state::ImportProgress {
                        phase,
                        done,
                        total,
                        filename,
                    });
                }
                ImportMsg::ThumbJob {
                    photo_id,
                    hash,
                    path,
                } => {
                    // Highest-priority thumbnails for the first few imported
                    // photos — generated concurrently with the rest of the import.
                    self.thumbs.enqueue(photo_id, &hash, &path);
                }
                ImportMsg::Done(res) => {
                    result = Some(*res);
                    finish = true;
                    break;
                }
            }
        }

        if let Some(p) = progress {
            self.state.import_progress = p;
        }
        if finish {
            self.state.import_running = false;
            if let Some(res) = result {
                match res {
                    Ok(outcome) => {
                        self.toast(ToastKind::Success, outcome.summary());
                        match &outcome {
                            crate::app::state::ImportResult::Add(o) => {
                                // Open the imported folder as the workspace.
                                let folder = o.folder.clone();
                                let sort = self.state.ws.sort;
                                let _ = self.state.open_workspace(&folder, sort);
                                self.needs_save = true;
                            }
                            crate::app::state::ImportResult::Copy(o) => {
                                // Open the target folder as the workspace.
                                let folder = o.target_dir.clone();
                                let sort = self.state.ws.sort;
                                let _ = self.state.open_workspace(&folder, sort);
                                self.needs_save = true;
                                // 清空存储卡 (PRD 6.7): after a fully-successful
                                // import, offer to move the copied card files to
                                // the recycle bin. Destructive → requires confirm.
                                if o.clear_card && !o.copied_sources.is_empty() {
                                    let n = o.copied_sources.len();
                                    let paths: Vec<std::path::PathBuf> = o
                                        .copied_sources
                                        .iter()
                                        .map(std::path::PathBuf::from)
                                        .collect();
                                    self.confirm = Some(ConfirmDialog {
                                        title: "清空存储卡".into(),
                                        text: format!(
                                            "导入完成。是否将卡中 {n} 张已成功导入的照片移入回收站？（仅成功导入的文件会被清除，失败/取消的文件保留在卡中）"
                                        ),
                                        confirm_label: "移入回收站".into(),
                                        danger: true,
                                        on_confirm: Box::new(move |app| {
                                            match crate::io::recycle::move_to_recycle_bin(&paths) {
                                                Ok(()) => app.toast(
                                                    ToastKind::Success,
                                                    format!("已将 {n} 张源文件移入回收站"),
                                                ),
                                                Err(e) => app.toast(
                                                    ToastKind::Error,
                                                    format!("清空存储卡失败：{e}"),
                                                ),
                                            }
                                        }),
                                    });
                                }
                            }
                        }
                        self.state.import_result = Some(Ok(outcome));
                        // Auto-close the import dialog so the workspace/preview
                        // are visible right away.
                        self.state.show_import = false;
                    }
                    Err(e) => {
                        self.state.import_result = Some(Err(e.clone()));
                        self.toast(ToastKind::Error, format!("导入失败：{e}"));
                    }
                }
            }
            self.import_rx = None;
        } else {
            self.import_rx = Some(rx);
        }
    }

    /// Persist workspace state to the DB (auto-save, PRD 11).
    pub fn save_workspace(&mut self) {
        let folder = self.state.ws.folder_path.clone();
        let index = self.state.ws.current_index as i64;
        let sort = self.state.ws.sort.code().to_string();
        let _ = db::workspace::save(
            &self.state.db,
            &WorkspaceState {
                current_folder_path: if folder.is_empty() { None } else { Some(folder) },
                current_index: index,
                current_sort: sort,
                filter_json: None,
                last_selected_id: self.state.ws.current().map(|p| p.id),
                last_save_time: String::new(),
                last_crash_marker: false,
                recent_folders_json: None,
            },
        );
        // Clearing the crash marker after a successful save.
        let _ = db::workspace::clear_crash(&self.state.db);
        self.last_autosave = std::time::Instant::now();
        self.needs_save = false;
    }

    fn maybe_autosave(&mut self) {
        if self.needs_save && self.last_autosave.elapsed().as_secs() >= 10 {
            self.save_workspace();
        }
    }

    /// Handle global keyboard shortcuts (PRD 7.2).
    fn handle_input(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};

        if self.state.show_import
            || self.state.show_settings
            || self.state.show_delete_box
            || self.state.show_crash_recovery
            || self.state.show_export
            || self.state.show_filter
            || self.confirm.is_some()
        {
            return;
        }
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        if self.state.ws.folder_path.is_empty() {
            return;
        }

        // Navigation — right/D/Space next, left/A prev.
        let next = ctx.input_mut(|i| {
            i.consume_key(Modifiers::NONE, Key::ArrowRight)
                || i.consume_key(Modifiers::NONE, Key::D)
                || i.consume_key(Modifiers::NONE, Key::Space)
        });
        if next {
            if self.state.step(1) {
                self.toast(ToastKind::Info, "已是最后一张");
            }
            self.needs_save = true;
            return;
        }
        let prev = ctx.input_mut(|i| {
            i.consume_key(Modifiers::NONE, Key::ArrowLeft)
                || i.consume_key(Modifiers::NONE, Key::A)
        });
        if prev {
            if self.state.step(-1) {
                self.toast(ToastKind::Info, "已是第一张");
            }
            self.needs_save = true;
            return;
        }

        // Home / End.
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Home)) {
            self.state.jump_to(0);
            self.toast(ToastKind::Info, "已跳到第 1 张");
            self.needs_save = true;
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::End)) {
            self.state.jump_to(self.state.ws.items.len().saturating_sub(1));
            self.toast(ToastKind::Info, format!("已跳到第 {} 张", self.state.ws.items.len()));
            self.needs_save = true;
            return;
        }

        // Undo / redo (single Q/E/U operations only, PRD 7.2).
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Z)) {
            if self.state.undo() {
                self.toast(ToastKind::Info, "已撤销");
                self.needs_save = true;
            }
            return;
        }
        if ctx.input_mut(|i| {
            i.consume_key(Modifiers::CTRL, Key::Y)
                || i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Z)
        }) {
            if self.state.redo() {
                self.toast(ToastKind::Info, "已重做");
                self.needs_save = true;
            }
            return;
        }

        // Select all / deselect (PRD 7.9.1).
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::A)) {
            self.state.select_all(true);
            self.toast(ToastKind::Info, format!("已全选 {} 张", self.state.ws.selected_count()));
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::A)) {
            self.state.select_all(false);
            self.toast(ToastKind::Info, "已取消全选");
            return;
        }
        // Esc: clear selection first, then exit 100% zoom (PRD 7.2 / 7.4).
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if self.state.clear_selection() {
                self.toast(ToastKind::Info, "已取消选择");
            } else if self.zoom_active {
                self.zoom_active = false;
            }
            return;
        }

        // Z: toggle 100% zoom (PRD 7.4). Re-entering restores the photo's
        // remembered pan anchor (PRD 7.4.1) and kicks off the full-resolution
        // RAW decode (PRD 7.4 视口解码) when not already cached.
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Z)) {
            self.zoom_active = !self.zoom_active;
            if self.zoom_active {
                if let Some(p) = self.state.ws.current().cloned() {
                    self.zoom_center = self
                        .zoom_anchors
                        .get(&p.id)
                        .copied()
                        .unwrap_or((0.5, 0.5));
                    self.request_zoom_decode(&p);
                }
            }
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Num0)) {
            self.zoom_active = false;
            self.toast(ToastKind::Info, "已重置为适配窗口");
            return;
        }

        // Ctrl+Q / Ctrl+E / Ctrl+U → batch apply to the selection (confirmed).
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Q)) {
            self.apply_batch_status(Status::Delete);
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::E)) {
            self.apply_batch_status(Status::Reviewed);
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::U)) {
            self.apply_batch_status(Status::Untreated);
            return;
        }

        // Single Q / E / U → current photo (undoable), then auto-advance.
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Q)) {
            let changed = self.state.set_status_current(Status::Delete, true).unwrap_or(false);
            self.needs_save = true;
            if changed {
                let blocked = self.state.step(1);
                if blocked {
                    self.toast(ToastKind::Warning, "已是最后一张");
                }
            }
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::E)) {
            let changed = self.state.set_status_current(Status::Reviewed, true).unwrap_or(false);
            self.needs_save = true;
            if changed {
                let blocked = self.state.step(1);
                if blocked {
                    self.toast(ToastKind::Warning, "已是最后一张");
                }
            }
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::U)) {
            let changed = self.state.set_status_current(Status::Untreated, true).unwrap_or(false);
            self.needs_save = true;
            let _ = changed;
            return;
        }

        // Ctrl+S save; Ctrl+I / Ctrl+O open import.
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::S)) {
            self.save_workspace();
            self.toast(ToastKind::Success, "工作区已保存");
            return;
        }
        if ctx.input_mut(|i| {
            i.consume_key(Modifiers::CTRL, Key::I) || i.consume_key(Modifiers::CTRL, Key::O)
        }) {
            self.state.show_import = true;
            return;
        }
    }

    /// Batch-apply a status to the current selection (Ctrl+Q/E/U, PRD 7.9.2).
    /// Confirms first when the setting is enabled; never goes into the undo stack.
    fn apply_batch_status(&mut self, status: Status) {
        let n = self.state.ws.selected_count();
        if n == 0 {
            self.toast(ToastKind::Warning, "请先选中要批量的照片（Ctrl+单击 / Ctrl+A）");
            return;
        }
        let label = match status {
            Status::Delete => "待删",
            Status::Reviewed => "已阅",
            Status::Untreated => "未处理",
        };
        if self.state.config.batch_confirm {
            let status_copy = status;
            self.confirm = Some(ConfirmDialog {
                title: "批量操作".into(),
                text: format!("将选中的 {n} 张照片标记为「{label}」？此操作不可撤销。"),
                confirm_label: "确认".into(),
                danger: status == Status::Delete,
                on_confirm: Box::new(move |app| {
                    match app.state.set_status_selected(status_copy) {
                        Ok(_) => app.toast(ToastKind::Success, format!("已将 {n} 张标记为「{label}」")),
                        Err(e) => app.toast(ToastKind::Error, format!("批量标记失败：{e}")),
                    }
                    app.needs_save = true;
                }),
            });
        } else {
            let r = self.state.set_status_selected(status);
            match r {
                Ok(_) => self.toast(ToastKind::Success, format!("已将 {n} 张标记为「{label}」")),
                Err(e) => self.toast(ToastKind::Error, format!("批量标记失败：{e}")),
            }
            self.needs_save = true;
        }
    }

    /// Start an add-mode import on a background thread, streaming progress
    /// messages back through `import_rx`.
    pub fn start_add_import(&mut self, path: &str) {
        let (tx, rx) = channel();
        let cancel = Arc::clone(&self.import_cancel);
        self.import_cancel.store(false, Ordering::SeqCst);
        let source = path.to_string();
        let recursive = self.import_recursive;
        let dedup = self.import_dedup;

        self.state.import_running = true;
        self.state.import_result = None;
        self.state.import_progress = crate::app::state::ImportProgress {
            phase: "扫描".to_string(),
            done: 0,
            total: 0,
            filename: "正在扫描文件…".to_string(),
        };
        self.import_rx = Some(rx);

        std::thread::spawn(move || {
            let auto_cancel = Arc::clone(&cancel);
            let tx_progress = tx.clone();
            // Request background thumbnail generation for the first N imported
            // photos so the very first thumbnails are ready when import finishes.
            let tx_thumb = tx.clone();
            let mut thumb_sent = 0usize;
            let res = (|| -> Result<import::ImportOutcome, String> {
                let mut db = Db::open_default().map_err(|e| e.to_string())?;
                let mut prog = move |phase: &str, done: usize, total: usize, name: &str| -> bool {
                    if auto_cancel.load(Ordering::SeqCst) {
                        return false;
                    }
                    let _ = tx_progress.send(ImportMsg::Progress {
                        phase: phase.to_string(),
                        done,
                        total,
                        filename: name.to_string(),
                    });
                    true
                };
                let mut on_thumb = move |photo_id: i64, hash: &str, path: &str| {
                    if thumb_sent < ADD_THUMB_PREWARM {
                        thumb_sent += 1;
                        let _ = tx_thumb.send(ImportMsg::ThumbJob {
                            photo_id,
                            hash: hash.to_string(),
                            path: path.to_string(),
                        });
                    }
                };
                import::add_mode_import_with_thumbs(
                    &mut db,
                    std::path::Path::new(&source),
                    recursive,
                    dedup,
                    &mut prog,
                    &mut on_thumb,
                )
                .map_err(|e| e.to_string())
            })();
            let _ = tx.send(ImportMsg::Done(Box::new(
                res.map(crate::app::state::ImportResult::Add),
            )));
        });
    }

    /// Start a copy-mode import on a background thread, creating an import
    /// session journal for resume (PRD 6.7.1). When `resume_from` is `Some`,
    /// the existing session journal is reused (so resume continues the same
    /// journal instead of creating a fresh one) and the copy progress is offset
    /// by the number of files already finished.
    pub fn start_copy_import(
        &mut self,
        source: &str,
        options: crate::app::copy::CopyOptions,
        resume_from: Option<crate::app::session::ImportSession>,
    ) {
        let (tx, rx) = channel();
        let cancel = Arc::clone(&self.import_cancel);
        self.import_cancel.store(false, Ordering::SeqCst);
        let source = source.to_string();
        let target = options.target_dir.clone();
        let org_code = options.org_mode.code().to_string();
        let recursive = options.recursive;
        let dedup = options.dedup;
        let clear_card = options.clear_card;
        // Number already finished this import (resume base), or 0 for a fresh run.
        let resume_base = resume_from.as_ref().map(|s| s.done).unwrap_or(0);
        let resume_flag = resume_from.is_some();

        self.state.import_running = true;
        self.state.import_result = None;
        self.state.import_progress = crate::app::state::ImportProgress {
            phase: "准备".to_string(),
            done: resume_base,
            total: 0,
            filename: "准备导入…".to_string(),
        };
        self.import_rx = Some(rx);

        std::thread::spawn(move || {
            let auto_cancel = Arc::clone(&cancel);
            let tx_progress = tx.clone();
            let res = (|| -> Result<crate::app::copy::CopyOutcome, String> {
                let mut db = Db::open_default().map_err(|e| e.to_string())?;
                // Reuse the existing journal on resume; otherwise create a new one.
                let session = Arc::new(std::sync::Mutex::new(match resume_from {
                    Some(s) => s,
                    None => crate::app::session::start(
                        &source,
                        &target,
                        &org_code,
                        recursive,
                        dedup,
                        0,
                    )
                    .map_err(|e| e.to_string())?,
                }));
                let sess = Arc::clone(&session);
                let mut prog = move |phase: &str, done: usize, total: usize, name: &str| -> bool {
                    if auto_cancel.load(Ordering::SeqCst) {
                        return false;
                    }
                    // Record progress in the journal (throttled).
                    if let Ok(mut s) = sess.lock() {
                        s.total = total;
                        s.done = done;
                        if done % 25 == 0 || done == total {
                            let _ = crate::app::session::write(&s);
                        }
                    }
                    let _ = tx_progress.send(ImportMsg::Progress {
                        phase: phase.to_string(),
                        done,
                        total,
                        filename: name.to_string(),
                    });
                    true
                };
                let opts = crate::app::copy::CopyOptions {
                    target_dir: target.clone(),
                    org_mode: crate::app::copy::OrgMode::from_code(&org_code),
                    recursive,
                    dedup,
                    clear_card,
                };
                let outcome = crate::app::copy::copy_mode_import(
                    &mut db,
                    std::path::Path::new(&source),
                    &opts,
                    resume_flag,
                    resume_base,
                    &mut prog,
                )
                .map_err(|e| e.to_string())?;
                // On explicit cancel, abandon the session; on a normal finish,
                // mark it completed (PRD 6.7.1).
                if let Ok(mut s) = session.lock() {
                    if cancel.load(Ordering::SeqCst) {
                        let _ = crate::app::session::abandon(&mut s);
                    } else {
                        s.total = outcome.scanned;
                        s.done = resume_base + outcome.copied;
                        let _ = crate::app::session::complete(&mut s);
                    }
                }
                Ok(outcome)
            })();
            let _ = tx.send(ImportMsg::Done(Box::new(
                res.map(crate::app::state::ImportResult::Copy),
            )));
        });
    }

    /// Handle a folder dropped onto the window → add-mode import (UI spec 3.3).
    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            let p = f.path().to_path_buf();
            if p.is_dir() {
                self.import_path = p.to_string_lossy().into_owned();
                self.state.show_import = true;
            }
        }
    }

    /// React to a newly inserted removable drive (SD card) by opening the copy
    /// import dialog with that drive as the source (PRD 6.2).
    fn handle_card(&mut self) {
        while let Some(ev) = self.card.poll() {
            let CardEvent::Inserted(letter) = ev;
            if !self.state.config.auto_detect_card {
                continue;
            }
            // Don't interrupt an active import or an open modal.
            if self.state.import_running
                || self.state.show_import
                || self.state.show_settings
                || self.state.show_crash_recovery
                || self.show_resume
            {
                continue;
            }
            self.import_mode = crate::app::state::ImportMode::Copy;
            self.import_path = format!("{letter}:\\");
            self.import_target = self.state.config.default_target_dir.clone();
            self.state.show_import = true;
        }
    }

    /// Populate recent folders for the top-bar path dropdown.
    pub fn recent_folders(&self) -> Vec<Folder> {
        db::folders::recent_folders(&self.state.db).unwrap_or_default()
    }

    /// Open the settings dialog, seeding the draft from the current config.
    pub fn open_settings(&mut self) {
        self.settings_draft = self.state.config.clone();
        self.state.show_settings = true;
    }

    /// Finalize crash recovery: clear the marker, persist and close the dialog.
    pub fn settle_crash_recovery(&mut self) {
        self.pending_crash = None;
        self.state.show_crash_recovery = false;
        self.save_workspace();
        self.toast(ToastKind::Success, "工作区已恢复");
    }

    /// Drain background thumbnail-completion events and invalidate texture
    /// cache entries so the fresh thumbnail/preview is loaded next frame.
    pub fn drain_thumbs(&mut self) {
        for (photo_id, hash) in self.thumbs.poll() {
            self.textures.invalidate(photo_id, &hash);
        }
    }

    // ---- Z-key full-resolution decode (PRD 7.4) ----

    /// The decoded full-resolution texture for a photo, if available. When it
    /// is missing this also queues the background decode (idempotent).
    pub fn zoom_texture(&mut self, item: &PhotoListItem) -> Option<egui::TextureHandle> {
        let hash = item.thumb_hash.clone().unwrap_or_default();
        let key = (item.id, hash);
        if let Some(tex) = self.zoom_tex.get(&key) {
            return Some(tex);
        }
        self.request_zoom_decode(item);
        None
    }

    /// Queue a full-resolution decode for `item` if it is eligible (RAW /
    /// plainly decodable, not flagged decode_failed, not already in flight).
    pub fn request_zoom_decode(&mut self, item: &PhotoListItem) {
        if item.decode_failed || self.zoom_worker.is_pending(item.id) {
            return;
        }
        if !zoom_full_decode_eligible(&item.current_path) {
            return;
        }
        self.zoom_worker.request(item.id, std::path::Path::new(&item.current_path));
    }

    /// 强制重试 RAW 解码 (PRD 7.4.3): clear the persisted decode_failed flag
    /// and retry once.
    pub fn retry_zoom_decode(&mut self, photo_id: i64) {
        let _ = db::photos::set_decode_failed(&self.state.db, photo_id, false);
        if let Some(p) = self.state.ws.items.iter().find(|p| p.id == photo_id).cloned() {
            if let Some(item) = self.state.ws.items.iter_mut().find(|p| p.id == photo_id) {
                item.decode_failed = false;
            }
            self.request_zoom_decode(&p);
            self.toast(ToastKind::Info, "正在重新解码 RAW…");
        }
    }

    /// Fold decode-worker messages into state: dimension hints update the 100%
    /// framing; finished decodes become textures in the 2 GB LRU; failures
    /// persist the decode_failed flag (PRD 7.4.3).
    pub fn poll_zoom(&mut self, ctx: &egui::Context) {
        for msg in self.zoom_worker.poll() {
            match msg {
                ZoomMsg::Dims {
                    photo_id,
                    width,
                    height,
                } => {
                    self.zoom_dims.insert(photo_id, (width, height));
                }
                ZoomMsg::Done { photo_id, result } => match result {
                    Ok(d) => {
                        self.zoom_dims.insert(photo_id, (d.width, d.height));
                        let Some(item) = self.state.ws.items.iter().find(|p| p.id == photo_id) else {
                            continue;
                        };
                        let hash = item.thumb_hash.clone().unwrap_or_default();
                        let img = egui::ColorImage::from_rgba_unmultiplied(
                            [d.width as usize, d.height as usize],
                            &d.rgba,
                        );
                        let tex = ctx.load_texture(
                            format!("zoom-{photo_id}"),
                            img,
                            egui::TextureOptions::LINEAR,
                        );
                        let bytes = (d.width as u64) * (d.height as u64) * 4;
                        self.zoom_tex.insert((photo_id, hash), tex, bytes);
                    }
                    Err(e) => {
                        log::warn!("RAW 解码失败 photo_id={photo_id}: {e}");
                        let _ = db::photos::set_decode_failed(&self.state.db, photo_id, true);
                        if let Some(item) = self
                            .state
                            .ws
                            .items
                            .iter_mut()
                            .find(|p| p.id == photo_id)
                        {
                            item.decode_failed = true;
                        }
                        self.toast(
                            ToastKind::Warning,
                            "RAW 解码失败，已标记为仅显示内嵌预览（右键可强制重试）",
                        );
                    }
                },
            }
        }
    }

    // ---- Disk-cache cleaner (PRD 9.4) ----

    /// Trigger an incremental clean when the user has browsed 50 photos or the
    /// app has idled for 60s ( whichever first), capped at 100 files per run.
    fn maybe_cache_clean(&mut self) {
        if self.cache_clean_running {
            return;
        }
        if self.photos_since_clean >= 50 || self.last_clean_at.elapsed() >= CACHE_CLEAN_IDLE {
            self.photos_since_clean = 0;
            self.last_clean_at = std::time::Instant::now();
            self.start_cache_clean(CACHE_CLEAN_MAX_FILES, false);
        }
    }

    /// Spawn a background cleanup pass. `max_files = usize::MAX` for the
    /// settings-page 「立即清理」 (`full = true` reports a completion toast).
    pub fn start_cache_clean(&mut self, max_files: usize, full: bool) {
        if self.cache_clean_running {
            return;
        }
        self.cache_clean_running = true;
        self.cache_clean_full = full;
        let cap = self
            .state
            .config
            .cache_capacity_gb
            .saturating_mul(1024 * 1024 * 1024);
        let expire = self.state.config.cache_expire_days;
        let progress = Arc::clone(&self.cache_clean_progress);
        let (tx, rx) = channel();
        self.cache_clean_rx = Some(rx);
        std::thread::spawn(move || {
            progress.store(0, Ordering::SeqCst);
            let res = (|| -> anyhow::Result<crate::io::cache_clean::CleanStats> {
                let idx = crate::io::cache_index::CacheIndex::open_default()?;
                let dir = crate::paths::cache_dir();
                crate::io::cache_clean::reconcile(&idx, &dir);
                let mut prog = |done: usize| -> bool {
                    progress.store(done, Ordering::SeqCst);
                    true
                };
                crate::io::cache_clean::run_cleanup(&idx, &dir, cap, expire, max_files, &mut prog)
            })();
            let _ = tx.send(res);
        });
    }

    /// Non-blocking drain of cleanup results. Small cleans stay silent
    /// (PRD 9.4 边用边删); a settings-triggered full clean toasts the outcome.
    fn poll_cache_clean(&mut self) {
        let Some(rx) = &self.cache_clean_rx else {
            return;
        };
        if let Ok(res) = rx.try_recv() {
            self.cache_clean_rx = None;
            self.cache_clean_running = false;
            let full = self.cache_clean_full;
            self.cache_clean_full = false;
            match res {
                Ok(s) => {
                    log::info!(
                        "缓存清理完成：删除 {} 个（过期 {}），释放 {}",
                        s.deleted,
                        s.expired,
                        crate::app::copy::human_bytes(s.freed_bytes as i64)
                    );
                    if full {
                        self.toast(
                            ToastKind::Success,
                            format!(
                                "缓存清理完成：删除 {} 个文件（过期 {}），释放 {}",
                                s.deleted,
                                s.expired,
                                crate::app::copy::human_bytes(s.freed_bytes as i64)
                            ),
                        );
                    }
                }
                Err(e) => {
                    if full {
                        self.toast(ToastKind::Error, format!("缓存清理失败：{e}"));
                    } else {
                        log::warn!("后台缓存清理失败: {e}");
                    }
                }
            }
        }
    }

    /// Enqueue background generation for every photo in the workspace whose
    /// thumbnail + preview caches are missing (PRD 9.3 / 3.1).
    pub fn enqueue_workspace_missing(&mut self) {
        let folder = self.state.ws.folder_path.clone();
        if folder.is_empty() {
            return;
        }
        let items = db::photos::list_items_in_folder(&self.state.db, &folder, self.state.ws.sort)
            .unwrap_or_default();
        for p in &items {
            if let Some(hash) = &p.thumb_hash {
                let t = crate::io::thumbnails::thumb_path(hash, 1.0);
                let pv = crate::io::thumbnails::preview_path(hash);
                if !t.exists() && !pv.exists() {
                    self.thumbs.enqueue(p.id, hash, &p.current_path);
                }
            }
        }
    }

    fn render(&mut self, ui: &mut egui::Ui) {
        view::render(self, ui);
        dialogs::render_dialogs(self, ui.ctx());
    }
}

impl Drop for KakaApp {
    /// Persist the workspace on a normal close so "自动打开上次工作区" works on the
    /// next launch (a hard kill / crash skips this and is handled by the crash
    /// marker instead).
    fn drop(&mut self) {
        if self.state.folder_loaded {
            let _ = self.save_workspace();
        }
    }
}

impl eframe::App for KakaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_drops(&ctx);
        self.handle_card();
        self.poll_import();
        self.poll_zoom(&ctx);
        self.poll_cache_clean();
        self.handle_input(&ctx);

        // Enqueue missing thumb caches once per workspace.
        if self.state.ws.folder_path != self.last_ws_folder {
            self.last_ws_folder = self.state.ws.folder_path.clone();
            self.enqueue_workspace_missing();
        }
        self.drain_thumbs();

        // Track viewed-photo count for the incremental cache clean (PRD 9.4).
        let cur_id = self.state.ws.current().map(|p| p.id);
        if cur_id != self.last_viewed_id {
            self.last_viewed_id = cur_id;
            if cur_id.is_some() {
                self.photos_since_clean += 1;
            }
        }
        self.maybe_cache_clean();

        self.render(ui);
        self.maybe_autosave();
        self.expire_toasts();
        ctx.request_repaint();
    }
}

/// Whether the Z-key 100% view should attempt a full-resolution decode of this
/// file: RAW (rawler develop) plus the formats `image` can decode directly.
/// HEIC/HEIF is excluded — without the system codec there is nothing to decode,
/// so it stays preview-only instead of being marked decode_failed.
fn zoom_full_decode_eligible(path: &str) -> bool {
    use crate::io::format::{classify, Classification, FormatKind};
    let p = std::path::Path::new(path);
    if !p.exists() {
        return false;
    }
    matches!(
        classify(p),
        Classification::Photo(FormatKind::Raw)
            | Classification::Photo(FormatKind::Jpeg)
            | Classification::Photo(FormatKind::Png)
            | Classification::Photo(FormatKind::Tiff)
    )
}
