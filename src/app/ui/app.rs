//! The eframe application shell: startup, event loop, layout orchestration.

use super::super::{import, state::AppState};
use super::texture::TextureCache;
use super::{dialogs, theme, view};
use crate::app::card::{CardDetector, CardEvent};
use crate::app::memcache::MemLru;
use crate::app::thumbs::ThumbWorker;
use crate::app::zoom::{ZoomMsg, ZoomWorker};
use crate::i18n::{self, t};
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

    // Import pre-scan (PRD 6.5 第三步 去重扫描 / UI 5.1-4 文件网格).
    /// Scan results + per-path checkbox state survive rescans of the same set.
    pub import_scan_files: Vec<crate::app::import::PrescanItem>,
    pub import_scan_selected: std::collections::HashSet<String>,
    pub import_scan_running: bool,
    pub import_scan_rx: Option<Receiver<anyhow::Result<Option<Vec<crate::app::import::PrescanItem>>>>>,
    pub import_scan_cancel: Arc<AtomicBool>,
    pub import_scan_done: Arc<AtomicUsize>,
    pub import_scan_total: Arc<AtomicUsize>,
    /// (path, recursive) the current/last scan was started for; a change
    /// re-triggers the scan (debounced for typed path edits).
    pub import_scan_key: Option<(String, bool)>,
    pub import_scan_dirty_since: Option<f64>,
    /// Grid toolbar state (UI 5.1-4).
    pub import_scan_sort: ImportScanSort,
    pub import_scan_filter: ImportScanFilter,
    /// 0 = S, 1 = M, 2 = L (see IMPORT_SCAN_CELL_SIZES).
    pub import_scan_cell: usize,
    /// Grid scroll offset bookkeeping: keep the view position across S/M/L
    /// cell-size switches (content height changes drastically).
    pub import_scan_last_cell: usize,
    pub import_scan_scroll_offset: f32,
    /// DEBUG: last logged grid layout signature (jitter diagnosis).
    pub import_scan_dbg_sig: (usize, i32, i32, i32, i32),
    /// Expanded detail list of the import completion report (PRD 6.8).
    pub import_report_view: Option<ImportReportView>,
    /// 稍后处理 (UI 3.3.1): hide the missing-file overlay for this photo until
    /// another photo is selected.
    pub missing_overlay_dismissed: Option<i64>,
    /// 后台预解码 (PRD 9.5): ±1 邻居的预览纹理预载 worker（结果经 poll 领取）。
    pub preview_preload: crate::app::preload::PreloadWorker,
    /// 预览缓存文件缺失的 (id, hash)，本会话内不再尝试预载。
    pub preload_skip: std::collections::HashSet<(i64, String)>,

    // Zoom (Z-key) view state (PRD 7.4). The pan anchor is stored as the image
    // point (fractions 0..1) shown at the viewport center, so it survives the
    // preview -> RAW texture swap unchanged (无缝替换).
    pub zoom_active: bool,
    pub zoom_center: (f32, f32),
    /// Target state the animated display values (`zoom_scale`/`zoom_center`)
    /// ease toward. The wheel writes targets only; easing turns the per-detent
    /// steps and clamp/fit handoffs into a smooth glide instead of jumps.
    pub zoom_scale_target: f32,
    pub zoom_center_target: (f32, f32),
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

    // 待删框 (PRD 8.1): multi-select scoped to the delete-box grid — separate
    // from the workspace selection — plus the Shift+click range anchor.
    pub delete_sel: std::collections::HashSet<i64>,
    pub delete_anchor: Option<usize>,

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
    /// Cache usage snapshot (total bytes, file count) taken when settings open.
    pub cache_usage: Option<(i64, i64)>,

    /// Digit-jump buffer (PRD 4.8.2): accumulated 0-9 digits; Enter jumps,
    /// Esc/2s idle cancels.
    pub digit_buffer: String,
    pub digit_started: std::time::Instant,
    /// Borderless fullscreen state (F11 toggles, Esc exits).
    pub fullscreen: bool,
    /// Free zoom multiplier on top of the 1:1 baseline (Ctrl+滚轮, PRD 7.4
    /// 补充). Reset to 1.0 whenever Z re-enters the 100% view.
    pub zoom_scale: f32,
    /// Custom-key capture in progress: the action code being rebound
    /// (设置 → 快捷键). While set, the settings dialog owns the keyboard.
    pub kb_capture: Option<String>,
    /// Last capture error (conflict / reserved key), shown under the grid.
    pub kb_error: Option<String>,

    /// Cache-root migration in flight (PRD 9.2): background copy old → new.
    pub cache_migrating: bool,
    pub cache_migrate_rx: Option<Receiver<anyhow::Result<usize>>>,

    /// Full cache rebuild in flight (PRD 9.6): regenerate every thumbnail +
    /// preview from the DB on a background thread, cancellable at any time.
    pub cache_rebuilding: bool,
    /// (processed, failed, cancelled) sent when the thread finishes.
    pub cache_rebuild_rx: Option<Receiver<anyhow::Result<(usize, usize, bool)>>>,
    pub cache_rebuild_cancel: Arc<AtomicBool>,
    /// Live progress for the settings button label 重建中 (x/N).
    pub cache_rebuild_done: Arc<AtomicUsize>,
    pub cache_rebuild_total: Arc<AtomicUsize>,

    /// Copy export (PRD 12.1) running on a background thread so the UI never
    /// freezes; progress is shared via atomics + a mutex'd current filename.
    pub export_copy_running: bool,
    pub export_copy_rx: Option<Receiver<anyhow::Result<crate::app::export::ExportOutcome>>>,
    pub export_copy_cancel: Arc<AtomicBool>,
    pub export_copy_progress: Arc<ExportCopyProgress>,
    /// Finished copy outcome for display inside the export dialog.
    pub export_copy_result: Option<Result<ExportCopyReport, String>>,
}

/// Shared copy-export progress (PRD 12.1), written by the worker thread and
/// read by the export dialog every frame.
#[derive(Default)]
pub struct ExportCopyProgress {
    pub current: std::sync::Mutex<String>,
    pub done: AtomicUsize,
    pub total: AtomicUsize,
}

/// Outcome of a finished copy export, shown in the export dialog.
#[derive(Clone)]
pub struct ExportCopyReport {
    pub copied: usize,
    pub failed: usize,
    pub cancelled: bool,
    /// Files attempted (copied + failed) for the failure-list header.
    pub total: usize,
    pub failures: Vec<crate::app::import::ImportFailure>,
}

/// Sort key of the import pre-scan grid (UI 5.1-4 工具栏).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportScanSort {
    CaptureTime,
    Filename,
    Size,
}

/// Filter of the import pre-scan grid (UI 5.1-4 工具栏).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportScanFilter {
    All,
    ToImport,
    Exists,
}

/// Cell size presets of the import pre-scan grid: S / M / L.
pub const IMPORT_SCAN_CELL_SIZES: [f32; 3] = [100.0, 140.0, 180.0];

/// Which detail list is expanded in the import completion report (PRD 6.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportReportView {
    Failures,
    Repairs,
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
            import_scan_files: Vec::new(),
            import_scan_selected: std::collections::HashSet::new(),
            import_scan_running: false,
            import_scan_rx: None,
            import_scan_cancel: Arc::new(AtomicBool::new(false)),
            import_scan_done: Arc::new(AtomicUsize::new(0)),
            import_scan_total: Arc::new(AtomicUsize::new(0)),
            import_scan_key: None,
            import_scan_dirty_since: None,
            import_scan_sort: ImportScanSort::CaptureTime,
            import_scan_filter: ImportScanFilter::All,
            import_scan_cell: 1,
            import_scan_last_cell: 1,
            import_scan_scroll_offset: 0.0,
            import_scan_dbg_sig: (0, 0, 0, 0, 0),
            import_report_view: None,
            missing_overlay_dismissed: None,
            preview_preload: crate::app::preload::PreloadWorker::new(),
            preload_skip: std::collections::HashSet::new(),
            zoom_active: false,
            zoom_center: (0.5, 0.5),
            zoom_scale_target: 1.0,
            zoom_center_target: (0.5, 0.5),
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
            delete_sel: std::collections::HashSet::new(),
            delete_anchor: None,
            cache_clean_rx: None,
            cache_clean_running: false,
            cache_clean_full: false,
            cache_clean_progress: Arc::new(AtomicUsize::new(0)),
            photos_since_clean: 0,
            last_viewed_id: None,
            last_clean_at: std::time::Instant::now(),
            cache_usage: None,
            digit_buffer: String::new(),
            digit_started: std::time::Instant::now(),
            fullscreen: false,
            zoom_scale: 1.0,
            kb_capture: None,
            kb_error: None,
            cache_migrating: false,
            cache_migrate_rx: None,
            cache_rebuilding: false,
            cache_rebuild_rx: None,
            cache_rebuild_cancel: Arc::new(AtomicBool::new(false)),
            cache_rebuild_done: Arc::new(AtomicUsize::new(0)),
            cache_rebuild_total: Arc::new(AtomicUsize::new(0)),
            export_copy_running: false,
            export_copy_rx: None,
            export_copy_cancel: Arc::new(AtomicBool::new(false)),
            export_copy_progress: Arc::new(ExportCopyProgress::default()),
            export_copy_result: None,
        };
        if app.startup.first_run {
            app.toast(
                ToastKind::Info,
                t("欢迎使用咔咔！只做导入+筛选。点击「导入」开始添加照片。",
                  "Welcome to Kaka! Import + cull only. Click Import to add your first photos."),
            );
        }
        if app.startup.corruption_detected {
            app.state.show_db_corruption = true;
        }
        app
    }
}

/// Launch the GUI. This owns init, and blocks until the window closes.
pub fn run() -> anyhow::Result<()> {
    // File logging + panic hook first, so everything after it is captured.
    crate::logging::init();

    // 1. Config (cache override before ensure_dirs, PRD 9.2, so the user's
    // cache folder is the one created/used from the first frame on).
    let cfg = config::load();
    i18n::set_lang(i18n::Lang::from_code(&cfg.language));
    crate::paths::set_cache_override(if cfg.cache_dir.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(cfg.cache_dir.trim()))
    });
    crate::paths::ensure_dirs()?;

    // 2. Database open + integrity + migration.
    let (db, startup) = init_database()?;

    // 3. Crash marker bookkeeping (skipped while the DB is corrupt — the
    // corruption dialog decides the fate of the database first).
    let was_crash = if startup.corruption_detected {
        false
    } else {
        db::workspace::crash_marker(&db)?
    };
    if !startup.corruption_detected {
        db::workspace::mark_crash(&db)?;
    }

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
            // A corrupt database takes priority: the three-button repair dialog
            // runs before any resume/crash/auto-open logic.
            if !app.startup.corruption_detected {
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
    // Lenient open: if the file cannot even be opened (corrupt header from a
    // text-editor edit), keep the real path on an in-memory placeholder so the
    // three-button repair dialog can still restore/reset it.
    let mut db = match Db::open_default() {
        Ok(db) => db,
        Err(e) => {
            log::error!("打开数据库失败（可能损坏）: {e}");
            startup.corruption_detected = true;
            Db::placeholder_at_default()
        }
    };

    // Integrity check (PRD 10.6). On failure, do NOT auto-repair: the app shows
    // the three-button dialog (自动修复 / 手动选备份 / 放弃新建) so the user
    // decides. init/migrate are skipped until the DB has been repaired.
    if !db.integrity_check().unwrap_or(false) {
        startup.corruption_detected = true;
        log::error!("数据库完整性检查失败，等待用户选择修复方式");
    }
    if startup.corruption_detected {
        return Ok((db, startup));
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
                                        title: t("清空存储卡", "Clear memory card").into(),
                                        text: match i18n::lang() {
                                            i18n::Lang::Zh => format!(
                                                "导入完成。是否将卡中 {n} 张已成功导入的照片移入回收站？（仅成功导入的文件会被清除，失败/取消的文件保留在卡中）"
                                            ),
                                            i18n::Lang::En => format!(
                                                "Import finished. Move the {n} successfully imported files on the card to the recycle bin? (Only fully imported files are cleared; failed/skipped files stay on the card.)"
                                            ),
                                        },
                                        confirm_label: t("移入回收站", "Move to recycle bin").into(),
                                        danger: true,
                                        on_confirm: Box::new(move |app| {
                                            match crate::io::recycle::move_to_recycle_bin(&paths) {
                                                Ok(()) => {
                                                    let msg = match i18n::lang() {
                                                        i18n::Lang::Zh => format!("已将 {n} 张源文件移入回收站"),
                                                        i18n::Lang::En => format!("Moved {n} source files to the recycle bin"),
                                                    };
                                                    app.toast(ToastKind::Success, msg);
                                                }
                                                Err(e) => app.toast(
                                                    ToastKind::Error,
                                                    format!("{}{e}", t("清空存储卡失败：", "Clear memory card failed: ")),
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

    /// Handle global keyboard shortcuts (PRD 7.2). Remappable actions go
    /// through the user's bindings (设置 → 快捷键); reserved keys are fixed.
    fn handle_input(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};

        // While capturing a custom binding the settings dialog owns the keyboard.
        if self.kb_capture.is_some() {
            return;
        }
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }

        let modal_open = self.state.show_import
            || self.state.show_settings
            || self.state.show_delete_box
            || self.state.show_crash_recovery
            || self.state.show_export
            || self.state.show_filter
            || self.state.show_db_corruption
            || self.confirm.is_some();

        // Esc chain: cancel digit jump → close dialog → exit fullscreen →
        // clear selection → leave 100% zoom (PRD 7.2). Crash/resume dialogs
        // need an explicit choice and are deliberately not ESC-dismissable.
        if crate::app::keybinds::consume_key_exact(ctx, Modifiers::NONE, Key::Escape) {
            if !self.digit_buffer.is_empty() {
                self.digit_buffer.clear();
            } else if self.confirm.is_some() {
                self.confirm = None;
            } else if self.state.show_import {
                self.state.show_import = false;
            } else if self.state.show_settings {
                self.state.show_settings = false;
                self.kb_capture = None;
                self.kb_error = None;
            } else if self.state.show_filter {
                self.state.show_filter = false;
            } else if self.state.show_export {
                self.state.show_export = false;
            } else if self.state.show_delete_box {
                self.state.show_delete_box = false;
            } else if self.fullscreen {
                self.set_fullscreen(ctx, false);
            } else if self.state.clear_selection() {
                self.toast(ToastKind::Info, t("已取消选择", "Selection cleared"));
            } else if self.zoom_active {
                self.zoom_active = false;
            }
            return;
        }

        // F11: fullscreen toggle (reserved).
        if crate::app::keybinds::consume_key_exact(ctx, Modifiers::NONE, Key::F11) {
            self.set_fullscreen(ctx, !self.fullscreen);
            return;
        }

        if modal_open {
            return;
        }

        // Digit jump: 0-9 accumulate into a buffer, Enter jumps, 2s idle
        // cancels (PRD 4.8.2 数字跳片). Collected in event order so fast
        // typing that lands several digits in one frame is not lost; digits
        // may arrive as Key::NumX or as Text events (numpad/IME paths).
        // One physical press arrives as BOTH a Key event and a Text event
        // (the WM_CHAR echo), so collect them separately: prefer Key digits
        // and only fall back to Text digits when no Key digit was seen —
        // otherwise "5" would be recorded twice.
        let mut key_digits: Vec<char> = Vec::new();
        let mut text_digits: Vec<char> = Vec::new();
        let mut enter_pressed = false;
        ctx.input(|i| {
            for e in &i.events {
                match e {
                    egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => {
                        if *key == Key::Enter {
                            enter_pressed = true;
                        } else if modifiers.is_none() {
                            if let Some(ch) = digit_key_char(*key) {
                                key_digits.push(ch);
                            }
                        }
                    }
                    egui::Event::Text(txt)
                        if txt.len() == 1 && txt.as_bytes()[0].is_ascii_digit() =>
                    {
                        text_digits.push(txt.as_bytes()[0] as char);
                    }
                    _ => {}
                }
            }
        });
        let new_digits = if !key_digits.is_empty() { key_digits } else { text_digits };
        if !new_digits.is_empty() {
            for ch in new_digits {
                if self.digit_buffer.chars().count() < 6 {
                    self.digit_buffer.push(ch);
                }
            }
            self.digit_started = std::time::Instant::now();
        } else if !self.digit_buffer.is_empty()
            && self.digit_started.elapsed() >= std::time::Duration::from_secs(2)
        {
            self.digit_buffer.clear();
        }
        if enter_pressed && !self.digit_buffer.is_empty() {
            let n: usize = self.digit_buffer.parse().unwrap_or(0);
            self.digit_buffer.clear();
            let len = self.state.ws.items.len();
            if len > 0 && n >= 1 {
                let target = (n - 1).min(len - 1);
                self.state.jump_to(target);
                let shown = target + 1;
                let msg = match i18n::lang() {
                    i18n::Lang::Zh => format!("已跳到第 {shown} 张"),
                    i18n::Lang::En => format!("Jumped to photo {shown}"),
                };
                self.toast(ToastKind::Info, msg);
                self.needs_save = true;
            }
            return;
        }

        if self.state.ws.folder_path.is_empty() {
            return;
        }

        // Navigation (remappable).
        if self.fire(ctx, "next_photo") {
            if self.advance(1) {
                self.toast(ToastKind::Info, t("已是最后一张", "Already at the last photo"));
            }
            self.needs_save = true;
            return;
        }
        if self.fire(ctx, "prev_photo") {
            if self.advance(-1) {
                self.toast(ToastKind::Info, t("已是第一张", "Already at the first photo"));
            }
            self.needs_save = true;
            return;
        }

        // Home / End (reserved).
        if crate::app::keybinds::consume_key_exact(ctx, Modifiers::NONE, Key::Home) {
            self.state.jump_to(0);
            self.toast(ToastKind::Info, t("已跳到第 1 张", "Jumped to photo 1"));
            self.needs_save = true;
            return;
        }
        if crate::app::keybinds::consume_key_exact(ctx, Modifiers::NONE, Key::End) {
            self.state.jump_to(self.state.ws.items.len().saturating_sub(1));
            let n = self.state.ws.items.len();
            let msg = match i18n::lang() {
                i18n::Lang::Zh => format!("已跳到第 {n} 张"),
                i18n::Lang::En => format!("Jumped to photo {n}"),
            };
            self.toast(ToastKind::Info, msg);
            self.needs_save = true;
            return;
        }

        // Undo / redo (remappable; Ctrl+Shift+Z stays a reserved redo alias).
        if self.fire(ctx, "undo") {
            if self.state.undo() {
                self.toast(ToastKind::Info, t("已撤销", "Undone"));
                self.needs_save = true;
            }
            return;
        }
        if self.fire(ctx, "redo")
            || crate::app::keybinds::consume_key_exact(ctx, Modifiers::CTRL | Modifiers::SHIFT, Key::Z)
        {
            if self.state.redo() {
                self.toast(ToastKind::Info, t("已重做", "Redone"));
                self.needs_save = true;
            }
            return;
        }

        // Select all (remappable) / deselect (reserved).
        if self.fire(ctx, "select_all") {
            self.state.select_all(true);
            let n = self.state.ws.selected_count();
            let msg = match i18n::lang() {
                i18n::Lang::Zh => format!("已全选 {n} 张"),
                i18n::Lang::En => format!("Selected all {n}"),
            };
            self.toast(ToastKind::Info, msg);
            return;
        }
        if crate::app::keybinds::consume_key_exact(ctx, Modifiers::CTRL | Modifiers::SHIFT, Key::A) {
            self.state.select_all(false);
            self.toast(ToastKind::Info, t("已取消全选", "Deselected all"));
            return;
        }

        // Z: toggle 100% zoom (remappable); Ctrl+0 reset (reserved).
        if self.fire(ctx, "toggle_zoom") {
            self.zoom_active = !self.zoom_active;
            if self.zoom_active {
                // Re-entering the 100% view always restarts at the 1:1
                // baseline; Ctrl+滚轮 then adjusts freely from there. Both the
                // animated values and their targets are snapped so Z feels
                // instant (wheel easing state must not carry over).
                self.zoom_scale = 1.0;
                self.zoom_scale_target = 1.0;
                if let Some(p) = self.state.ws.current().cloned() {
                    self.zoom_center = self
                        .zoom_anchors
                        .get(&p.id)
                        .copied()
                        .unwrap_or((0.5, 0.5));
                    self.zoom_center_target = self.zoom_center;
                    self.request_zoom_decode(&p);
                }
            }
            return;
        }
        if crate::app::keybinds::consume_key_exact(ctx, Modifiers::CTRL, Key::Num0) {
            self.zoom_active = false;
            self.toast(ToastKind::Info, t("已重置为适配窗口", "Reset to fit"));
            return;
        }

        // Rotation (PRD 7.2 / 4.7): R remappable; Ctrl+R / Shift+R reserved.
        if self.fire(ctx, "rotate_cw") {
            self.rotate_current(1);
            return;
        }
        if crate::app::keybinds::consume_key_exact(ctx, Modifiers::CTRL, Key::R) {
            self.rotate_current(-1);
            return;
        }
        if crate::app::keybinds::consume_key_exact(ctx, Modifiers::SHIFT, Key::R) {
            self.rotate_current(0);
            return;
        }

        // Ctrl+Q / Ctrl+E / Ctrl+U → batch apply to the selection (reserved).
        if crate::app::keybinds::consume_key_exact(ctx, Modifiers::CTRL, Key::Q) {
            self.apply_batch_status(Status::Delete);
            return;
        }
        if crate::app::keybinds::consume_key_exact(ctx, Modifiers::CTRL, Key::E) {
            self.apply_batch_status(Status::Reviewed);
            return;
        }
        if crate::app::keybinds::consume_key_exact(ctx, Modifiers::CTRL, Key::U) {
            self.apply_batch_status(Status::Untreated);
            return;
        }

        // Single marks (remappable) → current photo; Q/E auto-advance.
        if self.fire(ctx, "mark_delete") {
            let changed = self.state.set_status_current(Status::Delete, true).unwrap_or(false);
            self.needs_save = true;
            if changed && self.advance(1) {
                self.toast(ToastKind::Warning, t("已是最后一张", "Already at the last photo"));
            }
            return;
        }
        if self.fire(ctx, "mark_reviewed") {
            let changed = self.state.set_status_current(Status::Reviewed, true).unwrap_or(false);
            self.needs_save = true;
            if changed && self.advance(1) {
                self.toast(ToastKind::Warning, t("已是最后一张", "Already at the last photo"));
            }
            return;
        }
        if self.fire(ctx, "mark_untreated") {
            let changed = self.state.set_status_current(Status::Untreated, true).unwrap_or(false);
            self.needs_save = true;
            let _ = changed;
            return;
        }

        // Toggle right info panel (remappable, PRD 7.2 / UI spec 2.3).
        if self.fire(ctx, "toggle_panel") {
            self.state.right_panel_visible = !self.state.right_panel_visible;
            self.toast(
                ToastKind::Info,
                if self.state.right_panel_visible {
                    t("已展开信息面板", "Info panel shown")
                } else {
                    t("已折叠信息面板", "Info panel hidden")
                },
            );
            return;
        }

        // Ctrl+S save (remappable); Ctrl+I / Ctrl+O import (reserved).
        if self.fire(ctx, "save") {
            self.save_workspace();
            self.toast(ToastKind::Success, t("工作区已保存", "Workspace saved"));
            return;
        }
        if crate::app::keybinds::consume_key_exact(ctx, Modifiers::CTRL, Key::I)
            || crate::app::keybinds::consume_key_exact(ctx, Modifiers::CTRL, Key::O) {
            self.state.show_import = true;
            return;
        }
    }

    /// Advance/recede by `delta`, honoring 筛选到末尾循环跳张 (wrap_at_end,
    /// PRD 4.1). Returns true when blocked at a boundary (wrap disabled).
    pub fn advance(&mut self, delta: i64) -> bool {
        let wrap = self.state.config.wrap_at_end;
        let before = self.state.ws.current_index as i64;
        let blocked = self.state.step(delta, wrap);
        if !blocked && wrap {
            let after = self.state.ws.current_index as i64;
            let wrapped = (delta > 0 && after < before) || (delta < 0 && after > before);
            if wrapped {
                let msg = if delta > 0 {
                    t("已循环到第 1 张", "Wrapped to photo 1").to_string()
                } else {
                    let n = self.state.ws.items.len();
                    match i18n::lang() {
                        i18n::Lang::Zh => format!("已循环到第 {n} 张"),
                        i18n::Lang::En => format!("Wrapped to photo {n}"),
                    }
                };
                self.toast(ToastKind::Info, msg);
            }
        }
        blocked
    }

    /// True if the current binding of `action` was pressed this frame.
    fn fire(&self, ctx: &egui::Context, action: &str) -> bool {
        for code in crate::app::keybinds::effective_codes(&self.state.config.keybindings, action) {
            if crate::app::keybinds::consume(ctx, &code) {
                return true;
            }
        }
        false
    }

    /// Rotate the current photo by quarter turns (PRD 7.2/4.7): DB-only
    /// display rotation, never touches files, not undoable (PRD 7.2 例外).
    fn rotate_current(&mut self, delta: i64) {
        match self.state.rotate_current(delta) {
            Ok(Some(_)) => {}
            Ok(None) => return,
            Err(e) => {
                self.toast(
                    ToastKind::Error,
                    format!("{}{e}", t("旋转失败：", "Rotation failed: ")),
                );
                return;
            }
        }
        self.needs_save = true;
        let msg = if delta == 0 {
            t("已重置为 EXIF 方向", "Reset to EXIF orientation").to_string()
        } else if delta > 0 {
            t("已顺时针旋转 90°", "Rotated 90° clockwise").to_string()
        } else {
            t("已逆时针旋转 90°", "Rotated 90° counter-clockwise").to_string()
        };
        self.toast(ToastKind::Info, msg);
    }

    /// Toggle borderless fullscreen (F11 in, Esc out).
    fn set_fullscreen(&mut self, ctx: &egui::Context, on: bool) {
        self.fullscreen = on;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
        self.toast(
            ToastKind::Info,
            if on {
                t("已进入全屏（F11 / Esc 退出）", "Fullscreen (F11 or Esc to exit)")
            } else {
                t("已退出全屏", "Exited fullscreen")
            },
        );
    }

    /// Batch-apply a status to the current selection (Ctrl+Q/E/U, PRD 7.9.2).
    /// Confirms first when the setting is enabled; never goes into the undo stack.
    fn apply_batch_status(&mut self, status: Status) {
        let n = self.state.ws.selected_count();
        if n == 0 {
            self.toast(ToastKind::Warning, t("请先选中要批量的照片（Ctrl+单击 / Ctrl+A）", "Select photos first (Ctrl+click / Ctrl+A)"));
            return;
        }
        let label = match status {
            Status::Delete => t("待删", "delete"),
            Status::Reviewed => t("已阅", "reviewed"),
            Status::Untreated => t("未处理", "unprocessed"),
        };
        if self.state.config.batch_confirm {
            let status_copy = status;
            self.confirm = Some(ConfirmDialog {
                title: "批量操作".into(),
                text: match i18n::lang() {
                    i18n::Lang::Zh => format!("将选中的 {n} 张照片标记为「{label}」？此操作不可撤销。"),
                    i18n::Lang::En => format!("Mark {n} selected photo(s) as '{label}'? This cannot be undone."),
                },
                confirm_label: t("确认", "Confirm").into(),
                danger: status == Status::Delete,
                on_confirm: Box::new(move |app| {
                    match app.state.set_status_selected(status_copy) {
                        Ok(_) => {
                            let msg = match i18n::lang() {
                                i18n::Lang::Zh => format!("已将 {n} 张标记为「{label}」"),
                                i18n::Lang::En => format!("Marked {n} photo(s) as '{label}'"),
                            };
                            app.toast(ToastKind::Success, msg);
                        }
                        Err(e) => app.toast(ToastKind::Error, format!("{}{e}", t("批量标记失败：", "Batch marking failed: "))),
                    }
                    app.needs_save = true;
                }),
            });
        } else {
            let r = self.state.set_status_selected(status);
            match r {
                Ok(_) => {
                    let msg = match i18n::lang() {
                        i18n::Lang::Zh => format!("已将 {n} 张标记为「{label}」"),
                        i18n::Lang::En => format!("Marked {n} photo(s) as '{label}'"),
                    };
                    self.toast(ToastKind::Success, msg);
                }
                Err(e) => self.toast(ToastKind::Error, format!("{}{e}", t("批量标记失败：", "Batch marking failed: "))),
            }
            self.needs_save = true;
        }
    }

    // ---- 文件丢失记录移除 (PRD 7.9.2 / UI 3.3.1) ----

    /// Remove a lost photo's LIBRARY RECORD (PRD 7.9.2): deletes the DB row
    /// and cleans the memory textures + disk-cache registration. Never
    /// touches files on disk.
    pub fn remove_missing_record(&mut self, photo_id: i64) {
        let Some(item) = self.state.ws.items.iter().find(|p| p.id == photo_id).cloned() else {
            return;
        };
        let _ = db::photos::delete_photo(&self.state.db, photo_id);
        if let Some(hash) = &item.thumb_hash {
            crate::io::cache_index::delete_hash_registrations(hash);
            self.textures.invalidate(photo_id, hash);
        }
        self.state.ws.remove_item(photo_id);
        let _ = self.state.refresh_counts();
        self.needs_save = true;
    }

    /// Remove every MISSING photo in the current selection (PRD 7.9.2).
    pub fn remove_missing_in_selection(&mut self) {
        let ids: Vec<i64> = self
            .state
            .ws
            .items
            .iter()
            .filter(|p| p.is_missing() && self.state.ws.selection.contains(&p.id))
            .map(|p| p.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let n = ids.len();
        for id in ids {
            self.remove_missing_record(id);
        }
        let msg = match i18n::lang() {
            i18n::Lang::Zh => format!("已移除 {n} 条丢失记录（仅数据库，不动磁盘文件）"),
            i18n::Lang::En => format!("Removed {n} missing records (library only, no files touched)"),
        };
        self.toast(ToastKind::Success, msg);
    }

    // ---- Import pre-scan (PRD 6.5 第三步 去重扫描) ----

    /// Scan the source folder on a background thread and mark every file
    /// 待导入/已存在/路径修复 via the three-element comparison. Cancellable.
    pub fn start_import_prescan(&mut self, path: String, recursive: bool) {
        if self.import_scan_running || self.state.import_running {
            return;
        }
        self.import_scan_running = true;
        self.import_scan_files.clear();
        self.import_scan_selected.clear();
        self.import_scan_cancel = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&self.import_scan_cancel);
        self.import_scan_done.store(0, Ordering::SeqCst);
        self.import_scan_total.store(0, Ordering::SeqCst);
        let done = Arc::clone(&self.import_scan_done);
        let total = Arc::clone(&self.import_scan_total);
        let (tx, rx) = channel();
        self.import_scan_rx = Some(rx);
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<Option<Vec<crate::app::import::PrescanItem>>> {
                let mut db = Db::open_default()?;
                crate::app::import::prescan_mark(
                    &mut db,
                    std::path::Path::new(&path),
                    recursive,
                    &mut |d, t| {
                        done.store(d, Ordering::SeqCst);
                        total.store(t, Ordering::SeqCst);
                        !cancel.load(Ordering::SeqCst)
                    },
                )
            })();
            let _ = tx.send(res);
        });
    }

    /// Non-blocking drain of pre-scan results; seeds the grid checkbox state
    /// (待导入 default-checked, 已存在 unchecked; user picks survive rescans).
    fn poll_import_prescan(&mut self) {
        let Some(rx) = &self.import_scan_rx else {
            return;
        };
        if let Ok(res) = rx.try_recv() {
            self.import_scan_rx = None;
            self.import_scan_running = false;
            match res {
                Ok(Some(items)) => {
                    let prev = std::mem::take(&mut self.import_scan_selected);
                    for it in &items {
                        let sel = prev.contains(&it.path) || it.mark == crate::app::import::PrescanMark::New;
                        if sel {
                            self.import_scan_selected.insert(it.path.clone());
                        }
                    }
                    self.import_scan_files = items;
                }
                Ok(None) => {} // cancelled by the user
                Err(e) => self.toast(
                    ToastKind::Error,
                    format!("{}{e}", t("预扫描失败：", "Pre-scan failed: ")),
                ),
            }
        }
    }

    /// Start an add-mode import on a background thread, streaming progress
    /// messages back through `import_rx`. `only` restricts the import to the
    /// pre-scan-grid-checked paths (PRD 6.5); None imports the whole folder.
    pub fn start_add_import(&mut self, path: &str, only: Option<Vec<String>>) {
        let (tx, rx) = channel();
        let cancel = Arc::clone(&self.import_cancel);
        self.import_cancel.store(false, Ordering::SeqCst);
        let source = path.to_string();
        let recursive = self.import_recursive;
        let dedup = self.import_dedup;

        self.state.import_running = true;
        self.state.import_result = None;
        self.import_report_view = None;
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
                    only.as_deref(),
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
        only: Option<Vec<String>>,
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
        self.import_report_view = None;
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
                    only.as_deref(),
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
                || self.state.show_db_corruption
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
        // Best-effort cache usage snapshot for the settings dialog.
        self.cache_usage = crate::io::cache_index::CacheIndex::open_default().ok().and_then(|idx| {
            let size = idx.total_size().ok()?;
            let count = idx.count().ok()?;
            Some((size, count))
        });
        self.state.show_settings = true;
    }

    /// Finalize crash recovery: clear the marker, persist and close the dialog.
    pub fn settle_crash_recovery(&mut self) {
        self.pending_crash = None;
        self.state.show_crash_recovery = false;
        self.save_workspace();
        self.toast(ToastKind::Success, "工作区已恢复");
    }

    // ---- 数据库维护（PRD 10.6 三按钮弹窗 / UI 5.3.4 设置-数据库） ----

    /// Finish DB recovery: close the corruption dialog, clear the flag, toast,
    /// and try to reopen the last workspace on the repaired database.
    fn db_repaired(&mut self, msg: String) {
        self.state.show_db_corruption = false;
        self.startup.corruption_detected = false;
        self.toast(ToastKind::Success, msg);
        if self.state.config.auto_open_last_workspace {
            if let Ok(Some(saved)) = db::workspace::load(&self.state.db) {
                if let Some(folder) = saved.current_folder_path {
                    let sort = SortOrder::from_code(&saved.current_sort);
                    let _ = self.state.open_workspace(&folder, sort);
                    self.state.ws.current_index = saved.current_index.max(0) as usize;
                }
            }
        }
    }

    /// Run a repair action then ensure schema init + migration.
    fn run_db_recovery(
        &mut self,
        action: impl FnOnce(&mut Db) -> anyhow::Result<()>,
        ok_msg: String,
    ) {
        let res = (|| -> anyhow::Result<()> {
            action(&mut self.state.db)?;
            db::schema::init(&mut self.state.db)?;
            db::schema::migrate(&mut self.state.db)?;
            Ok(())
        })();
        match res {
            Ok(()) => self.db_repaired(ok_msg),
            Err(e) => self.toast(ToastKind::Error, format!("数据库修复失败：{e}")),
        }
    }

    /// 损坏弹窗：自动修复（优先恢复最近可用备份，否则新建）。
    pub fn db_auto_repair(&mut self) {
        let msg = t(
            "数据库已自动修复（优先恢复最近可用备份）",
            "Database auto-repaired (nearest valid backup restored)",
        )
        .to_string();
        self.run_db_recovery(|db| db::schema::repair_or_reset(db), msg);
    }

    /// 损坏弹窗：放弃损坏库，新建空数据库。
    pub fn db_reset_fresh(&mut self) {
        let msg = t(
            "已放弃损坏的数据库，新建空数据库",
            "Corrupt database discarded; created a fresh empty one",
        )
        .to_string();
        self.run_db_recovery(|db| db::schema::reset_to_fresh(db), msg);
    }

    /// 损坏弹窗：从用户选择的备份恢复。
    pub fn db_restore_from(&mut self, path: &std::path::Path) {
        let msg = format!("已从备份恢复：{}", path.display());
        let p = path.to_path_buf();
        self.run_db_recovery(move |db| db::schema::restore_backup(db, &p), msg);
    }

    /// 设置 → 数据库：完整性检查。
    pub fn check_db_integrity(&mut self) {
        match self.state.db.integrity_check() {
            Ok(true) => self.toast(
                ToastKind::Success,
                t("数据库完整性正常", "Database integrity OK"),
            ),
            Ok(false) => self.toast(
                ToastKind::Error,
                t(
                    "数据库完整性检查失败（数据库可能损坏）",
                    "Integrity check failed (database may be corrupt)",
                ),
            ),
            Err(e) => self.toast(ToastKind::Error, format!("完整性检查出错：{e}")),
        }
    }

    /// 设置 → 数据库：手动备份。
    pub fn manual_db_backup(&mut self) {
        match db::schema::manual_backup(&self.state.db) {
            Ok(p) => self.toast(ToastKind::Success, format!("已备份到：{}", p.display())),
            Err(e) => self.toast(ToastKind::Error, format!("备份失败：{e}")),
        }
    }

    /// 设置 → 数据库：从用户选择的备份恢复（替换当前库并重载工作区）。
    pub fn restore_db_pick(&mut self, path: &std::path::Path) {
        let p = path.to_path_buf();
        let res = (|| -> anyhow::Result<()> {
            db::schema::restore_backup(&mut self.state.db, &p)?;
            db::schema::init(&mut self.state.db)?;
            db::schema::migrate(&mut self.state.db)?;
            Ok(())
        })();
        match res {
            Ok(()) => {
                self.state.undo_stack.clear();
                self.state.redo_stack.clear();
                self.state.histograms.clear();
                let _ = self.state.reload_current();
                self.toast(ToastKind::Success, format!("已从备份恢复：{}", path.display()));
            }
            Err(e) => self.toast(ToastKind::Error, format!("恢复失败：{e}")),
        }
    }

    /// Drain background thumbnail-completion events and invalidate texture
    /// cache entries so the fresh thumbnail/preview is loaded next frame.
    /// 后台预解码 (PRD 9.5): 空闲时把当前照片 ±1 邻居的 1920px 预览图解码
    /// 进内存纹理缓存。仅无导入/无扫描/无导出、非 Z 放大且无 Z 解码任务时
    /// 运行；每次重建 ±1 队列（自动取消不再需要的任务），已缓存/已尝试失败
    /// 的跳过。
    fn maybe_preload_neighbors(&mut self) {
        if self.state.import_running || self.import_scan_running || self.export_copy_running {
            return;
        }
        if self.zoom_active || self.zoom_worker.has_pending() {
            return; // Z 放大/RAW 解码进行中：暂停预读，让出 IO。
        }
        let idx = self.state.ws.current_index;
        let len = self.state.ws.items.len();
        if len == 0 {
            return;
        }
        let mut jobs = Vec::new();
        for delta in [1, -1] {
            let ni = idx as isize + delta;
            if ni < 0 || ni >= len as isize {
                continue;
            }
            let p = &self.state.ws.items[ni as usize];
            let Some(hash) = &p.thumb_hash else { continue };
            if hash.is_empty() {
                continue;
            }
            let key = (p.id, hash.clone());
            if self.preload_skip.contains(&key) || self.preview_preload.is_queued(&key) {
                continue;
            }
            if self.textures.preview_cached(p.id, hash) {
                continue; // 已在缓存，不重复解码
            }
            if !crate::app::preload::preview_cache_exists(hash) {
                // 磁盘缓存文件尚未生成（交给工作区缩略图管线），本次跳过。
                continue;
            }
            jobs.push(crate::app::preload::PreloadJob {
                photo_id: p.id,
                hash: hash.clone(),
                path: p.current_path.clone(),
            });
        }
        // 每帧整体重建队列：照片切换后陈旧任务自动被取消。
        self.preview_preload.set_queue(jobs);
    }

    /// 领取预解码结果并直接上传进预览纹理 MemLru（egui 纹理需 UI 线程上传）。
    fn poll_preload(&mut self, ctx: &egui::Context) {
        for done in self.preview_preload.poll() {
            match done.image {
                Some(img) => {
                    let tex = ctx.load_texture(
                        format!("preload-{}", done.photo_id),
                        img,
                        super::texture::photo_texture_options(),
                    );
                    self.textures.insert_preview(done.photo_id, &done.hash, tex);
                }
                None => {
                    self.preload_skip
                        .insert((done.photo_id, done.hash.clone()));
                }
            }
        }
    }

    pub fn drain_thumbs(&mut self) {
        // At most 2 per frame: each invalidation triggers a synchronous
        // texture reload; spreading them keeps the UI hitch-free while
        // batches of thumbnails finish (e.g. the import grid prewarm).
        for (photo_id, hash) in self.thumbs.poll_limited(2) {
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
        if !zoom_full_decode_eligible(&item.current_path) {
            return;
        }
        // Seed the dimension hint synchronously (header-only EXIF read, once
        // per photo) so the VERY FIRST zoom frame already frames at the RAW's
        // true size. Without it the preview shows at its own size — often the
        // 256px thumbnail when the 1920px preview cache is not generated yet —
        // until the worker's Dims message lands a few frames later, which
        // reads as a jarring tiny → stretched → sharp double jump. Also
        // benefits decode_failed photos, which never get a worker Dims msg.
        if !self.zoom_dims.contains_key(&item.id) {
            if let Some((w, h)) =
                crate::io::exif::pixel_dims(std::path::Path::new(&item.current_path))
            {
                self.zoom_dims.insert(item.id, (w, h));
            }
        }
        if item.decode_failed || self.zoom_worker.is_pending(item.id) {
            return;
        }
        let hash = item.thumb_hash.clone().unwrap_or_default();
        self.zoom_worker
            .request(item.id, std::path::Path::new(&item.current_path), &hash);
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
            self.toast(ToastKind::Info, t("正在重新解码 RAW…", "Retrying RAW decode…"));
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
                            super::texture::photo_texture_options(),
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
                            t("RAW 解码失败，已标记为仅显示内嵌预览（右键可强制重试）",
                              "RAW decode failed — marked preview-only (right-click to force retry)"),
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

    /// Migrate the whole cache directory to a new root on a background
    /// thread (PRD 9.2): copy → reset the global index handle → remove old.
    pub fn start_cache_migration(&mut self, old: std::path::PathBuf, new: std::path::PathBuf) {
        if self.cache_migrating {
            return;
        }
        if self.cache_rebuilding {
            // A rebuild writing into the old tree would lose its output when
            // the migration deletes it — don't overlap them.
            self.toast(
                ToastKind::Warning,
                t("正在重建缓存，请等重建结束（或取消）后再迁移缓存路径",
                  "A cache rebuild is running — finish or cancel it before migrating the cache path"),
            );
            return;
        }
        self.cache_migrating = true;
        self.toast(
            ToastKind::Info,
            t("正在迁移缓存到新路径…", "Migrating cache to the new path…"),
        );
        let (tx, rx) = channel();
        self.cache_migrate_rx = Some(rx);
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<usize> {
                let n = crate::io::cache_clean::migrate_cache(&old, &new)?;
                // Reopen the index at the new root before deleting the old tree.
                crate::io::cache_index::reset_global();
                std::fs::remove_dir_all(&old)?;
                Ok(n)
            })();
            let _ = tx.send(res);
        });
    }

    /// Non-blocking drain of migration results.
    fn poll_cache_migrate(&mut self) {
        let Some(rx) = &self.cache_migrate_rx else {
            return;
        };
        if let Ok(res) = rx.try_recv() {
            self.cache_migrate_rx = None;
            self.cache_migrating = false;
            match res {
                Ok(n) => {
                    let msg = match i18n::lang() {
                        i18n::Lang::Zh => format!("缓存迁移完成：已复制 {n} 个文件并清理旧目录"),
                        i18n::Lang::En => format!("Cache migrated: {n} files copied, old folder removed"),
                    };
                    self.toast(ToastKind::Success, msg);
                }
                Err(e) => self.toast(
                    ToastKind::Error,
                    format!("{}{e}", t("缓存迁移失败：", "Cache migration failed: ")),
                ),
            }
        }
    }

    /// Full cache rebuild (PRD 9.6): walk every photo row in the DB and
    /// regenerate its thumbnail + preview caches on a background thread,
    /// overwriting existing files. Cancellable; live progress is shared via
    /// atomics so the settings button can show 重建中 (x/N) without blocking.
    pub fn start_cache_rebuild(&mut self) {
        if self.cache_rebuilding || self.cache_migrating {
            return;
        }
        self.cache_rebuilding = true;
        self.cache_rebuild_cancel = Arc::new(AtomicBool::new(false));
        self.cache_rebuild_done.store(0, Ordering::SeqCst);
        self.cache_rebuild_total.store(0, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.cache_rebuild_rx = Some(rx);
        let cancel = Arc::clone(&self.cache_rebuild_cancel);
        let done_counter = Arc::clone(&self.cache_rebuild_done);
        let total_counter = Arc::clone(&self.cache_rebuild_total);
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<(usize, usize, bool)> {
                let db = Db::open_default()?;
                let (done, failed) = crate::app::cache_rebuild::rebuild_all_caches(
                    &db,
                    1.0,
                    || cancel.load(Ordering::SeqCst),
                    |done, total| {
                        done_counter.store(done, Ordering::SeqCst);
                        total_counter.store(total, Ordering::SeqCst);
                    },
                );
                Ok((done, failed, cancel.load(Ordering::SeqCst)))
            })();
            let _ = tx.send(res);
        });
    }

    /// Non-blocking drain of rebuild results.
    fn poll_cache_rebuild(&mut self) {
        let Some(rx) = &self.cache_rebuild_rx else {
            return;
        };
        if let Ok(res) = rx.try_recv() {
            self.cache_rebuild_rx = None;
            self.cache_rebuilding = false;
            match res {
                Ok((done, failed, cancelled)) => {
                    if cancelled {
                        let msg = match i18n::lang() {
                            i18n::Lang::Zh => format!("重建已取消：已完成 {done} 张"),
                            i18n::Lang::En => format!("Rebuild cancelled: {done} photos done"),
                        };
                        self.toast(ToastKind::Warning, msg);
                    } else {
                        let msg = match i18n::lang() {
                            i18n::Lang::Zh => {
                                if failed > 0 {
                                    format!("重建完成，共 {done} 张，失败 {failed} 张（详情见日志）")
                                } else {
                                    format!("重建完成，共 {done} 张")
                                }
                            }
                            i18n::Lang::En => {
                                if failed > 0 {
                                    format!("Rebuild finished: {done} photos, {failed} failed (see log)")
                                } else {
                                    format!("Rebuild finished: {done} photos")
                                }
                            }
                        };
                        if failed == 0 {
                            self.toast(ToastKind::Success, msg);
                        } else {
                            self.toast(ToastKind::Warning, msg);
                        }
                    }
                }
                Err(e) => self.toast(
                    ToastKind::Error,
                    format!("{}{e}", t("缓存重建失败：", "Cache rebuild failed: ")),
                ),
            }
        }
    }

    // ---- Copy export on a background thread (PRD 12.1) ----

    /// Spawn a copy export so the UI never freezes. The disk-space pre-check
    /// (export_space_guard) runs inside the worker unchanged and fails fast
    /// through the same result channel.
    pub fn start_export_copy(&mut self, folder: String, target: String) {
        if self.export_copy_running {
            return;
        }
        let org = self.export_org;
        let space_guard = self.state.config.export_space_guard;
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(ExportCopyProgress::default());
        self.export_copy_running = true;
        self.export_copy_result = None;
        self.export_copy_rx = Some(rx);
        self.export_copy_cancel = Arc::clone(&cancel);
        self.export_copy_progress = Arc::clone(&progress);
        std::thread::spawn(move || {
            let res = (|| -> anyhow::Result<crate::app::export::ExportOutcome> {
                let db = Db::open_default()?;
                crate::app::export::export_kept_copy(
                    &db,
                    &folder,
                    &target,
                    org,
                    true,
                    true,
                    space_guard,
                    &mut |name, done, total| {
                        if let Ok(mut cur) = progress.current.lock() {
                            *cur = name.to_string();
                        }
                        progress.done.store(done, Ordering::SeqCst);
                        progress.total.store(total, Ordering::SeqCst);
                        !cancel.load(Ordering::SeqCst)
                    },
                )
            })();
            let _ = tx.send(res);
        });
    }

    /// Non-blocking drain of copy-export results.
    fn poll_export_copy(&mut self) {
        let Some(rx) = &self.export_copy_rx else {
            return;
        };
        if let Ok(res) = rx.try_recv() {
            self.export_copy_rx = None;
            self.export_copy_running = false;
            let cancelled = self.export_copy_cancel.load(Ordering::SeqCst);
            match res {
                Ok(out) => {
                    let report = ExportCopyReport {
                        copied: out.copied,
                        failed: out.failed,
                        cancelled,
                        total: out.total,
                        failures: out.failures.clone(),
                    };
                    let msg = if cancelled {
                        match i18n::lang() {
                            i18n::Lang::Zh => format!(
                                "导出已取消：已完成 {} 张",
                                report.copied
                            ),
                            i18n::Lang::En => {
                                format!("Export cancelled: {} photos done", report.copied)
                            }
                        }
                    } else {
                        match i18n::lang() {
                            i18n::Lang::Zh => format!(
                                "导出完成：成功 {} 张 / 失败 {} 张",
                                report.copied, report.failed
                            ),
                            i18n::Lang::En => format!(
                                "Export finished: {} copied, {} failed",
                                report.copied, report.failed
                            ),
                        }
                    };
                    if !cancelled && report.failed == 0 {
                        self.toast(ToastKind::Success, msg);
                    } else {
                        self.toast(ToastKind::Warning, msg);
                    }
                    self.export_copy_result = Some(Ok(report));
                }
                Err(e) => {
                    self.toast(
                        ToastKind::Error,
                        format!("{}{e}", t("导出失败：", "Export failed: ")),
                    );
                    self.export_copy_result = Some(Err(e.to_string()));
                }
            }
        }
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
                // Enqueue unless BOTH caches exist. Checking "both missing"
                // instead would skip every photo that has a thumbnail but no
                // preview — e.g. all of them after the import pre-scan grid
                // (thumb-only prewarm), so their previews never appeared.
                if !crate::io::thumbnails::caches_complete(hash, 1.0) {
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
        self.poll_cache_migrate();
        self.poll_cache_rebuild();
        self.poll_export_copy();
        self.poll_import_prescan();
        self.poll_preload(&ctx);
        self.maybe_preload_neighbors();
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

/// Whether the Z-key 100% view should attempt a full-resolution decode.
/// RAW only (rawler develop). Non-RAW photos zoom with the disk-cached
/// 1920px preview directly — full-size decoding of e.g. a stitched panorama
/// (tens of thousands of pixels) exceeds GPU texture limits / memory and
/// crashes the app.
/// The digit character for an egui number key, if any (数字跳片).
fn digit_key_char(key: egui::Key) -> Option<char> {
    let ch = match key {
        egui::Key::Num0 => '0',
        egui::Key::Num1 => '1',
        egui::Key::Num2 => '2',
        egui::Key::Num3 => '3',
        egui::Key::Num4 => '4',
        egui::Key::Num5 => '5',
        egui::Key::Num6 => '6',
        egui::Key::Num7 => '7',
        egui::Key::Num8 => '8',
        egui::Key::Num9 => '9',
        _ => return None,
    };
    Some(ch)
}

fn zoom_full_decode_eligible(path: &str) -> bool {
    use crate::io::format::{classify, Classification, FormatKind};
    let p = std::path::Path::new(path);
    if !p.exists() {
        return false;
    }
    matches!(classify(p), Classification::Photo(FormatKind::Raw))
}
