//! Modal dialogs: import, crash recovery, settings, delete box, confirm, toasts.

use super::app::{ConfirmDialog, KakaApp, ToastKind};
use crate::i18n::{self, t};
use super::theme;
use crate::model::{PhotoListItem, SortOrder, Status};
use crate::db;
use chrono::Datelike;
use eframe::egui::{self, Align2, RichText};
use std::collections::HashSet;
use std::sync::atomic::Ordering;

pub fn render_dialogs(app: &mut KakaApp, ctx: &egui::Context) {
    // 数据库损坏三按钮弹窗（PRD 10.6）优先级最高：先修库，其他一切延后。
    if app.state.show_db_corruption {
        db_corruption_dialog(app, ctx);
        return;
    }
    if app.confirm.is_some() {
        confirm_dialog(app, ctx);
    }
    if app.show_resume && app.pending_resume.is_some() {
        resume_dialog(app, ctx);
    } else if app.state.show_crash_recovery && app.pending_crash.is_some() {
        crash_recovery(app, ctx);
    } else if app.state.show_import {
        import_dialog(app, ctx);
    }
    if app.state.show_settings {
        settings_dialog(app, ctx);
    }
    if app.state.show_filter {
        filter_dialog(app, ctx);
    }
    if app.state.show_export {
        export_dialog(app, ctx);
    }
    if app.state.show_delete_box {
        delete_box(app, ctx);
    }
    // Digit-jump indicator: accumulated digits + cursor (PRD 4.8.2).
    if !app.digit_buffer.is_empty() {
        egui::Area::new(egui::Id::new("digit_jump"))
            .anchor(Align2::RIGHT_TOP, [-16.0, 64.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(egui::Color32::from_rgba_unmultiplied(30, 30, 30, 0xF2))
                    .stroke(egui::Stroke::new(1.0, theme::BORDER_2))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.add(
                            // Extend: the right-anchored area leaves ~no wrap
                            // width, which would stack digits vertically.
                            egui::Label::new(
                                RichText::new(format!("{}_", app.digit_buffer))
                                    .size(16.0)
                                    .strong()
                                    .color(theme::ACCENT),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    });
            });
    }
    render_toasts(app, ctx);
}

/// Interrupted-import resume prompt (PRD 6.7.1).
fn resume_dialog(app: &mut KakaApp, ctx: &egui::Context) {
    let Some(session) = app.pending_resume.clone() else {
        app.show_resume = false;
        return;
    };
    let total = session.total;
    let done = session.done;
    let source = session.source.clone();
    let target = session.target.clone();

    dim_backdrop(ctx);
    egui::Window::new(t("断点续传", "Resume import"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([520.0, 260.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            ui.label(RichText::new(t("检测到上次有未完成的导入任务", "An import was interrupted")).size(20.0).strong().color(theme::TEXT));
            ui.add_space(8.0);
            ui.label(
                RichText::new(match i18n::lang() {
                    i18n::Lang::Zh => format!("共 {total} 张，已完成 {done} 张。\n是否从断点继续导入到：{target}"),
                    i18n::Lang::En => format!("{done} of {total} files done.\nContinue importing to: {target}?"),
                })
                .size(14.0)
                .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add(primary_button(t("继续导入", "Continue import"))).clicked() {
                    let opts = session.copy_options();
                    // Reopen the import dialog showing copy mode so the user sees progress.
                    app.import_mode = crate::app::state::ImportMode::Copy;
                    app.import_path = source.clone();
                    app.import_target = target.clone();
                    app.import_org = opts.org_mode;
                    app.state.show_import = true;
                    app.start_copy_import(&source, opts, Some(session.clone()), None);
                    app.show_resume = false;
                    app.pending_resume = None;
                }
                if ui
                    .button(RichText::new(t("放弃本次任务", "Discard task")).color(theme::DELETE))
                    .clicked()
                {
                    let mut s = session;
                    let _ = crate::app::session::abandon(&mut s);
                    app.show_resume = false;
                    app.pending_resume = None;
                    app.toast(ToastKind::Info, t("已放弃本次未完成的导入任务", "Discarded the unfinished import"));
                }
            });
        });
}

/// Draw a dimmed full-screen backdrop so modal windows read as modal.
fn dim_backdrop(ctx: &egui::Context) {
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("kaka_modal_backdrop"),
    ));
    painter.rect_filled(screen, 0.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 0x66));
}

/// PRD 6.8: the scrollable failure detail table (源路径/目标路径/原因/大小/时间).
fn failures_table(
    ui: &mut egui::Ui,
    failures: &[crate::app::import::ImportFailure],
    show_target: bool,
) {
    let head = |ui: &mut egui::Ui, text: &str| {
        ui.label(RichText::new(text).size(11.0).strong().color(theme::TEXT_SECONDARY));
    };
    // 滚动条常显（用户可知下方还有内容），末列留白避免滚动条压住文字。
    let spacer = |ui: &mut egui::Ui| {
        ui.allocate_exact_size(egui::vec2(14.0, 1.0), egui::Sense::hover());
    };
    egui::ScrollArea::vertical()
        .max_height(120.0)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(ui, |ui| {
            egui::Grid::new("import_report_failures")
                .striped(true)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    head(ui, t("#", "#"));
                    head(ui, t("源路径", "Source"));
                    if show_target {
                        head(ui, t("目标路径", "Target"));
                    }
                    head(ui, t("原因", "Reason"));
                    head(ui, t("大小", "Size"));
                    head(ui, t("拍摄时间", "Capture time"));
                    spacer(ui);
                    ui.end_row();
                    for (i, f) in failures.iter().enumerate() {
                        ui.label(RichText::new(format!("{}", i + 1)).size(11.0));
                        ui.label(RichText::new(&f.source_path).size(11.0).color(theme::TEXT_WEAK));
                        if show_target {
                            ui.label(
                                RichText::new(f.target_path.as_str())
                                    .size(11.0)
                                    .color(theme::TEXT_WEAK),
                            );
                        }
                        ui.label(
                            RichText::new(format!("{}（{}）", f.reason_code, f.reason))
                                .size(11.0)
                                .color(theme::DELETE),
                        );
                        ui.label(
                            RichText::new(crate::app::copy::human_bytes(f.file_size)).size(11.0),
                        );
                        ui.label(RichText::new(&f.capture_time).size(11.0));
                        spacer(ui);
                        ui.end_row();
                    }
                });
        });
}

/// PRD 6.8: the scrollable path-repair detail table (旧路径→新路径).
fn repairs_table(ui: &mut egui::Ui, repairs: &[crate::app::import::PathRepair]) {
    let head = |ui: &mut egui::Ui, text: &str| {
        ui.label(RichText::new(text).size(11.0).strong().color(theme::TEXT_SECONDARY));
    };
    let spacer = |ui: &mut egui::Ui| {
        ui.allocate_exact_size(egui::vec2(14.0, 1.0), egui::Sense::hover());
    };
    egui::ScrollArea::vertical()
        .max_height(120.0)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(ui, |ui| {
            egui::Grid::new("import_report_repairs")
                .striped(true)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    head(ui, t("#", "#"));
                    head(ui, t("原始文件名", "Filename"));
                    head(ui, t("旧路径", "Old path"));
                    head(ui, t("新路径", "New path"));
                    head(ui, t("修复时间", "Repaired at"));
                    spacer(ui);
                    ui.end_row();
                    for (i, r) in repairs.iter().enumerate() {
                        ui.label(RichText::new(format!("{}", i + 1)).size(11.0));
                        ui.label(RichText::new(&r.original_filename).size(11.0));
                        ui.label(RichText::new(&r.old_path).size(11.0).color(theme::DELETE));
                        ui.label(RichText::new(&r.new_path).size(11.0).color(theme::KEEP));
                        ui.label(RichText::new(&r.repaired_at).size(11.0).color(theme::TEXT_WEAK));
                        spacer(ui);
                        ui.end_row();
                    }
                });
        });
}

fn import_dialog(app: &mut KakaApp, ctx: &egui::Context) {
    dim_backdrop(ctx);
    // 位置只计算一次（存入窗口状态），之后不再重算：anchor + 逐帧居中会让
    // egui 的两轮布局互相追逐，窗口整体左右 ±44px 振荡（抖动）。
    let px = ((ctx.input(|i| i.viewport_rect().width()) - 1160.0) / 2.0).max(8.0);
    let py = 40.0;
    // 窗口按状态取尺寸：无网格（扫描中/已取消/空）用紧凑尺寸，网格出现才
    // 展开——两种状态各自尺寸固定，不会重新引入逐帧的 auto-size 反馈回路。
    // 1160 宽让 S 档正好 10 列、M 档 7 列、L 档 6 列。
    let grid_visible = !app.state.import_running
        && !app.import_scan_running
        && !app.import_scan_files.is_empty();
    let win_size = if grid_visible { [1160.0, 700.0] } else { [720.0, 480.0] };
    egui::Window::new(t("导入照片", "Import Photos"))
        .collapsible(false)
        .resizable(false)
        .fixed_size(win_size)
        .default_pos(egui::pos2(px, py))
        .frame(dialog_frame())
        .show(ctx, |ui| {
            // Mode tabs (window title already says 导入 — no extra heading).
            ui.horizontal(|ui| {
                let add_sel = app.import_mode == crate::app::state::ImportMode::Add;
                if ui.selectable_label(add_sel, t("添加模式（从硬盘）", "Add (from disk)")).clicked() {
                    app.import_mode = crate::app::state::ImportMode::Add;
                }
                if ui.selectable_label(!add_sel, t("复制模式（从存储卡）", "Copy (from card)")).clicked() {
                    app.import_mode = crate::app::state::ImportMode::Copy;
                }
            });
            ui.separator();
            match app.import_mode {
                crate::app::state::ImportMode::Add => {
                    ui.label(
                        RichText::new(t("添加模式：将已有文件夹的照片添加到图库（仅索引，不拷贝文件）。", "Add mode: index photos from an existing folder on disk (nothing is copied)."))
                            .size(14.0)
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new(t("源路径", "Source folder")).size(13.0).color(theme::TEXT_SECONDARY));
                    ui.horizontal(|ui| {
                        let mut path = app.import_path.clone();
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut path)
                                .desired_width(520.0)
                                .hint_text(t("选择或粘贴要添加的文件夹路径", "Pick or paste a folder to add")),
                        );
                        if resp.changed() {
                            app.import_path = path;
                        }
                        if ui.button(t("浏览…", "Browse…")).clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                app.import_path = p.to_string_lossy().into_owned();
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(&mut app.import_recursive, t("递归扫描子文件夹", "Scan subfolders recursively"))
                            .changed()
                        {
                            // 递归开关变化立即重扫（绕过防抖）。
                            app.import_scan_dirty_since = Some(-1.0e9);
                        }
                        ui.checkbox(&mut app.import_dedup, t("去重扫描", "Dedup scan"));
                    });
                }
                crate::app::state::ImportMode::Copy => {
                    ui.label(
                        RichText::new(t("复制模式：从存储卡/文件夹导入，物理拷贝照片到目标目录，保留原文件名。", "Copy mode: copy photos from a card/folder to the target directory, keeping original filenames."))
                            .size(14.0)
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new(t("源路径", "Source folder")).size(13.0).color(theme::TEXT_SECONDARY));
                    ui.horizontal(|ui| {
                        let mut path = app.import_path.clone();
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut path)
                                .desired_width(480.0)
                                .hint_text(t("选择要导入的文件夹/存储卡", "Pick the folder / memory card to import")),
                        );
                        if resp.changed() {
                            app.import_path = path;
                        }
                        if ui.button(t("浏览…", "Browse…")).clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                app.import_path = p.to_string_lossy().into_owned();
                            }
                        }
                    });
                    ui.label(RichText::new(t("目标目录", "Target folder")).size(13.0).color(theme::TEXT_SECONDARY));
                    ui.horizontal(|ui| {
                        let mut target = app.import_target.clone();
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut target)
                                .desired_width(430.0)
                                .hint_text(t("选择目标目录", "Pick the target directory")),
                        );
                        if resp.changed() {
                            app.import_target = target;
                        }
                        if ui.button(t("浏览…", "Browse…")).clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                app.import_target = p.to_string_lossy().into_owned();
                            }
                        }
                    });
                    ui.label(RichText::new(t("子目录组织方式", "Subfolder layout")).size(13.0).color(theme::TEXT_SECONDARY));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut app.import_org, crate::app::copy::OrgMode::Structure, t("保持原结构", "Keep original structure"));
                        ui.radio_value(&mut app.import_org, crate::app::copy::OrgMode::Date, t("按拍摄日期", "By capture date"));
                        ui.radio_value(&mut app.import_org, crate::app::copy::OrgMode::Flat, t("全部平铺", "Flat"));
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(&mut app.import_recursive, t("递归扫描子文件夹", "Scan subfolders recursively"))
                            .changed()
                        {
                            // 递归开关变化立即重扫（绕过防抖）。
                            app.import_scan_dirty_since = Some(-1.0e9);
                        }
                        ui.checkbox(&mut app.import_dedup, t("去重扫描", "Dedup scan"));
                    });
                    // 清空存储卡 (PRD 6.3): only enabled when the source is a
                    // removable device; otherwise grayed out and forced off.
                    let removable =
                        crate::app::card::is_removable_source(std::path::Path::new(&app.import_path));
                    if !removable {
                        app.import_clear_card = false;
                    }
                    let checkbox = egui::Checkbox::new(&mut app.import_clear_card, t("导入后清空存储卡", "Clear card after import"));
                    let resp = ui.add_enabled(removable, checkbox).on_hover_text(t(
                        "导入完成且全部成功者会移入回收站（非永久删除）。仅当源路径为可移动存储设备时可用。",
                        "After a fully successful import the copied source files are moved to the recycle bin (not permanently deleted). Only enabled when the source is a removable device.",
                    ));
                    let _ = resp;
                }
            }
            ui.separator();

            // PRD 6.5 第三步 + UI 5.1-4: pre-scan stats, toolbar and grid.
            if !app.state.import_running {
                import_scan_area(app, ui, ctx);
            }

            if app.state.import_running {
                let p = app.state.import_progress.clone();
                let frac = if p.total > 0 { p.done as f32 / p.total as f32 } else { 0.0 };
                let phase_text = match p.phase.as_str() {
                    "检查" => t("正在检查与分析照片…", "Checking & analysing photos…"),
                    "拷贝" => t("正在拷贝照片…", "Copying photos…"),
                    "扫描" => t("正在扫描照片…", "Scanning photos…"),
                    "准备" => t("正在准备…", "Preparing…"),
                    other => other,
                };
                ui.label(
                    RichText::new(phase_text)
                        .size(13.0)
                        .strong()
                        .color(theme::ACCENT),
                );
                let bar_text = if p.filename.is_empty() {
                    format!("（{}/{}）", p.done, p.total)
                } else {
                    format!("{}（{}/{}）", p.filename, p.done, p.total)
                };
                ui.add(egui::ProgressBar::new(frac).text(bar_text));
                if ui.button(t("取消导入", "Cancel import")).clicked() {
                    app.import_cancel.store(true, Ordering::SeqCst);
                }
            } else {
                // 主按钮：导入 X 张（勾选且待导入的项）。
                let x = app
                    .import_scan_files
                    .iter()
                    .filter(|f| {
                        f.mark == crate::app::import::PrescanMark::New
                            && app.import_scan_selected.contains(&f.path)
                    })
                    .count();
                let label = match i18n::lang() {
                    i18n::Lang::Zh => format!("导入 {x} 张"),
                    i18n::Lang::En => format!("Import {x} photos"),
                };
                let btn = egui::Button::new(
                    RichText::new(label).size(15.0).strong().color(egui::Color32::from_rgb(0x12, 0x12, 0x12)),
                )
                .fill(theme::ACCENT)
                .stroke(egui::Stroke::new(1.0, theme::ACCENT));
                let enabled = x > 0 && !app.import_scan_running;
                if ui.add_enabled(enabled, btn).clicked() {
                    let path = app.import_path.trim().to_string();
                    // PRD 6.5: only checked + 待导入 items; add mode also auto
                    // includes path-repair candidates (PRD 6.4 maintenance).
                    let mut only: Vec<String> = app
                        .import_scan_files
                        .iter()
                        .filter(|f| {
                            f.mark == crate::app::import::PrescanMark::New
                                && app.import_scan_selected.contains(&f.path)
                        })
                        .map(|f| f.path.clone())
                        .collect();
                    match app.import_mode {
                        crate::app::state::ImportMode::Add => {
                            only.extend(
                                app.import_scan_files
                                    .iter()
                                    .filter(|f| f.mark == crate::app::import::PrescanMark::PathRepair)
                                    .map(|f| f.path.clone()),
                            );
                            app.start_add_import(&path, Some(only));
                        }
                        crate::app::state::ImportMode::Copy => {
                            if app.import_target.trim().is_empty() {
                                app.toast(ToastKind::Warning, t("请选择目标目录", "Pick a target folder first"));
                            } else {
                                // 复制模式勾选已存在 = 强制导入.
                                only.extend(
                                    app.import_scan_files
                                        .iter()
                                        .filter(|f| {
                                            f.mark == crate::app::import::PrescanMark::Exists
                                                && app.import_scan_selected.contains(&f.path)
                                        })
                                        .map(|f| f.path.clone()),
                                );
                                let opts = crate::app::copy::CopyOptions {
                                    target_dir: app.import_target.clone(),
                                    org_mode: app.import_org,
                                    recursive: app.import_recursive,
                                    dedup: app.import_dedup,
                                    clear_card: app.import_clear_card,
                                };
                                app.start_copy_import(&path, opts, None, Some(only));
                            }
                        }
                    }
                }
            }

            if let Some(res) = app.state.import_result.clone() {
                ui.separator();
                match res {
                    Ok(outcome) => {
                        match outcome {
                            crate::app::state::ImportResult::Add(o) => {
                                let msg = match i18n::lang() {
                                    i18n::Lang::Zh => format!("已将 {} 张照片添加到图库（跳过已存在 {} 张，失败 {} 张，路径修复 {} 条）", o.added, o.skipped_existing, o.failed, o.path_repaired),
                                    i18n::Lang::En => format!("Added {} photos ({} skipped as existing, {} failed, {} paths repaired)", o.added, o.skipped_existing, o.failed, o.path_repaired),
                                };
                                ui.label(RichText::new(msg).size(14.0).color(theme::KEEP));
                                use crate::app::ui::app::ImportReportView;
                                ui.horizontal(|ui| {
                                    // PRD 6.8: 展开失败 / 路径修复明细。
                                    if !o.failures.is_empty()
                                        && ui.button(t("查看失败列表", "View failure list")).clicked()
                                    {
                                        app.import_report_view = match app.import_report_view {
                                            Some(ImportReportView::Failures) => None,
                                            _ => Some(ImportReportView::Failures),
                                        };
                                    }
                                    if o.path_repaired > 0
                                        && ui.button(t("查看路径修复列表", "View path repairs")).clicked()
                                    {
                                        app.import_report_view = match app.import_report_view {
                                            Some(ImportReportView::Repairs) => None,
                                            _ => Some(ImportReportView::Repairs),
                                        };
                                    }
                                });
                                if !o.failures.is_empty()
                                    && app.import_report_view == Some(ImportReportView::Failures)
                                {
                                    failures_table(ui, &o.failures, false);
                                    ui.horizontal(|ui| {
                                        if ui
                                            .button(t("导出失败列表 .csv", "Export failures .csv"))
                                            .clicked()
                                        {
                                            if let Some(p) = rfd::FileDialog::new()
                                                .add_filter("CSV", &["csv"])
                                                .set_file_name("导入失败列表.csv")
                                                .save_file()
                                            {
                                                match crate::app::import::export_failure_csv(
                                                    &p.to_string_lossy(),
                                                    &o.failures,
                                                ) {
                                                    Ok(n) => app.toast(
                                                        ToastKind::Success,
                                                        format!("{}{n} {}", t("已导出 ", "Exported "), t("条失败记录", "failure records")),
                                                    ),
                                                    Err(e) => app.toast(
                                                        ToastKind::Error,
                                                        format!("{}{e}", t("导出失败：", "Export failed: ")),
                                                    ),
                                                }
                                            }
                                        }
                                    });
                                }
                                if o.path_repaired > 0
                                    && app.import_report_view == Some(ImportReportView::Repairs)
                                {
                                    repairs_table(ui, &o.repairs);
                                    ui.horizontal(|ui| {
                                        if ui
                                            .button(t("导出修复列表 .csv", "Export repairs .csv"))
                                            .clicked()
                                        {
                                            if let Some(p) = rfd::FileDialog::new()
                                                .add_filter("CSV", &["csv"])
                                                .set_file_name("路径修复列表.csv")
                                                .save_file()
                                            {
                                                match crate::app::import::export_repair_csv(
                                                    &p.to_string_lossy(),
                                                    &o.repairs,
                                                ) {
                                                    Ok(n) => app.toast(
                                                        ToastKind::Success,
                                                        format!("{}{n} {}", t("已导出 ", "Exported "), t("条修复记录", "repair records")),
                                                    ),
                                                    Err(e) => app.toast(
                                                        ToastKind::Error,
                                                        format!("{}{e}", t("导出失败：", "Export failed: ")),
                                                    ),
                                                }
                                            }
                                        }
                                    });
                                }
                            }
                            crate::app::state::ImportResult::Copy(o) => {
                                let msg = match i18n::lang() {
                                    i18n::Lang::Zh => format!("成功导入 {} 张（已存在跳过 {} 张，失败 {} 张）", o.copied, o.skipped_existing, o.failed),
                                    i18n::Lang::En => format!("Imported {} photos ({} skipped as existing, {} failed)", o.copied, o.skipped_existing, o.failed),
                                };
                                ui.label(RichText::new(msg).size(14.0).color(theme::KEEP));
                                if !o.failures.is_empty() {
                                    use crate::app::ui::app::ImportReportView;
                                    ui.horizontal(|ui| {
                                        if ui.button(t("查看失败列表", "View failure list")).clicked() {
                                            app.import_report_view = match app.import_report_view {
                                                Some(ImportReportView::Failures) => None,
                                                _ => Some(ImportReportView::Failures),
                                            };
                                        }
                                    });
                                    if app.import_report_view == Some(ImportReportView::Failures) {
                                        failures_table(ui, &o.failures, true);
                                        ui.horizontal(|ui| {
                                            if ui
                                                .button(t("导出失败列表 .csv", "Export failures .csv"))
                                                .clicked()
                                            {
                                                if let Some(p) = rfd::FileDialog::new()
                                                    .add_filter("CSV", &["csv"])
                                                    .set_file_name("导入失败列表.csv")
                                                    .save_file()
                                                {
                                                    match crate::app::import::export_failure_csv(
                                                        &p.to_string_lossy(),
                                                        &o.failures,
                                                    ) {
                                                        Ok(n) => app.toast(
                                                            ToastKind::Success,
                                                            format!("{}{n} {}", t("已导出 ", "Exported "), t("条失败记录", "failure records")),
                                                        ),
                                                        Err(e) => app.toast(
                                                            ToastKind::Error,
                                                            format!("{}{e}", t("导出失败：", "Export failed: ")),
                                                        ),
                                                    }
                                                }
                                            }
                                        });
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        ui.label(RichText::new(format!("{}{e}", t("导入失败：", "Import failed: "))).size(14.0).color(theme::DELETE));
                    }
                }
                if ui.button(t("关闭", "Close")).clicked() {
                    app.state.show_import = false;
                }
            } else {
                if ui.button(t("取消", "Cancel")).clicked() {
                    app.state.show_import = false;
                }
            }
        });
    ctx.request_repaint();
}

fn db_corruption_dialog(app: &mut KakaApp, ctx: &egui::Context) {
    dim_backdrop(ctx);
    egui::Window::new(t("数据库损坏", "Database Corrupt"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([560.0, 320.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            ui.label(
                RichText::new(t("检测到数据库损坏", "Database integrity check failed"))
                    .size(20.0)
                    .strong()
                    .color(theme::TEXT),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(t(
                    "启动时的完整性检查（PRAGMA integrity_check）未通过。请选择处理方式：",
                    "The startup integrity check failed. Please choose how to proceed:",
                ))
                .size(14.0)
                .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(12.0);

            let mut action: Option<DbRepairAction> = None;
            // 自动修复.
            if ui
                .add(egui::Button::new(
                    RichText::new(t("自动修复", "Auto Repair"))
                        .strong()
                        .color(egui::Color32::from_rgb(0x12, 0x12, 0x12)),
                ).fill(theme::ACCENT).stroke(egui::Stroke::new(1.0, theme::ACCENT)))
                .on_hover_text(t(
                    "优先恢复最近一次可用的自动备份；没有可用备份则新建空数据库。",
                    "Restore the nearest valid automatic backup, or create a fresh database if none is usable.",
                ))
                .clicked()
            {
                action = Some(DbRepairAction::AutoRepair);
            }
            // 手动选备份.
            if ui
                .add(egui::Button::new(RichText::new(
                    t("手动选择备份恢复…", "Restore From Backup…"),
                ).color(theme::TEXT)))
                .on_hover_text(t(
                    "从你手动选择的 .bak 备份文件恢复。",
                    "Restore from a .bak backup file you pick.",
                ))
                .clicked()
            {
                action = Some(DbRepairAction::PickBackup);
            }
            // 放弃新建.
            if ui
                .add(egui::Button::new(RichText::new(
                    t("放弃损坏库，新建空数据库", "Discard & Create Fresh Database"),
                ).color(theme::DELETE)))
                .on_hover_text(t(
                    "原损坏文件会被保留为 .corrupted_* 以便人工找回。",
                    "The corrupt file is kept as .corrupted_* for manual recovery.",
                ))
                .clicked()
            {
                action = Some(DbRepairAction::ResetFresh);
            }

            ui.add_space(8.0);
            ui.label(
                RichText::new(t(
                    "提示：损坏弹窗出现前若已有「手动备份」或「自动备份」，选择恢复是最稳妥的方式。",
                    "If you have any manual or automatic backup, restoring from it is the safest choice.",
                ))
                .size(12.0)
                .color(theme::TEXT_WEAK),
            );

            match action {
                Some(DbRepairAction::AutoRepair) => app.db_auto_repair(),
                Some(DbRepairAction::ResetFresh) => app.db_reset_fresh(),
                Some(DbRepairAction::PickBackup) => {
                    let start_dir = app
                        .state
                        .db
                        .path
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_default();
                    if let Some(p) = rfd::FileDialog::new()
                        .set_title(t("选择备份文件", "Select backup file"))
                        .add_filter("SQLite 备份", &["bak"])
                        .set_directory(start_dir)
                        .pick_file()
                    {
                        app.db_restore_from(&p);
                    }
                }
                None => {}
            }
        });
    ctx.request_repaint();
}

#[derive(Clone, Copy, PartialEq)]
enum DbRepairAction {
    AutoRepair,
    PickBackup,
    ResetFresh,
}

fn crash_recovery(app: &mut KakaApp, ctx: &egui::Context) {
    let Some(state) = app.pending_crash.clone() else {
        app.state.show_crash_recovery = false;
        return;
    };
    // Compute real counts for the last workspace (for the summary line).
    let counts = state
        .current_folder_path
        .clone()
        .and_then(|folder| db::photos::status_counts(&app.state.db, &folder).ok())
        .map(|c| (c.total, c.deleted + c.reviewed))
        .unwrap_or((0, 0));

    dim_backdrop(ctx);
    egui::Window::new(t("恢复", "Recovery"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([560.0, 340.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            ui.label(RichText::new(t("检测到上次非正常关闭", "Last session did not close properly")).size(20.0).strong().color(theme::TEXT));
            ui.add_space(8.0);
            let folder = state.current_folder_path.clone().unwrap_or_default();
            let (total, processed) = counts;
            ui.label(
                RichText::new(match i18n::lang() {
                    i18n::Lang::Zh => format!("上次工作区：{folder}\n已处理 {processed}/{total} 张。是否恢复上次的标记和浏览位置？"),
                    i18n::Lang::En => format!("Last workspace: {folder}\n{processed}/{total} processed. Restore your marks and browsing position?"),
                })
                .size(14.0)
                .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(6.0);
            ui.label(RichText::new(t("崩溃不会删除照片文件，仅可能丢失未保存的筛选标记。", "A crash never deletes photo files — only unsaved culling marks may be lost.")).size(12.0).color(theme::TEXT_WEAK));
            ui.add_space(10.0);

            let mut close = false;
            ui.horizontal(|ui| {
                if ui.add(primary_button(t("恢复并继续", "Resume where I left off"))).clicked() {
                    // Restore saved position.
                    if let Some(folder) = state.current_folder_path.clone() {
                        let sort = SortOrder::from_code(&state.current_sort);
                        let _ = app.state.open_workspace(&folder, sort);
                        app.state.ws.current_index = state.current_index.max(0) as usize;
                    }
                    app.settle_crash_recovery();
                    close = true;
                }
                if ui.button(t("保留标记，从头浏览", "Keep marks, browse from start")).clicked() {
                    if let Some(folder) = state.current_folder_path.clone() {
                        let sort = SortOrder::from_code(&state.current_sort);
                        let _ = app.state.open_workspace(&folder, sort);
                        app.state.ws.current_index = 0;
                    }
                    app.settle_crash_recovery();
                    close = true;
                }
                if ui.button(RichText::new(t("放弃所有未保存的标记，重置为未处理", "Discard all unsaved marks (reset to unprocessed)")).color(theme::DELETE)).clicked() {
                    // Reset all statuses in the last workspace to 0.
                    if let Some(folder) = state.current_folder_path.clone() {
                        if let Ok(ids) = db::photos::list_ids_in_folder(&app.state.db, &folder, SortOrder::CaptureTimeAsc) {
                            let _ = db::photos::set_status_batch(&app.state.db, &ids, Status::Untreated);
                        }
                        let sort = SortOrder::from_code(&state.current_sort);
                        let _ = app.state.open_workspace(&folder, sort);
                    }
                    app.settle_crash_recovery();
                    close = true;
                }
            });
            if close {
                app.state.show_crash_recovery = false;
            }
        });
}

fn settings_dialog(app: &mut KakaApp, ctx: &egui::Context) {
    // Edit a draft; only "保存" applies it to the live config and persists it.
    let mut save = false;
    // 设置 → 数据库：恢复备份（需要文件对话框，窗口后处理）。
    let mut restore_clicked = false;

    // Custom-key capture (PRD 7.2.1): read the first pressed key event this
    // frame. Esc cancels; reserved keys and conflicts keep the capture open
    // with a red hint (不允许保存，直到换键或先解除冲突).
    if let Some(action) = app.kb_capture.clone() {
        let ev = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => {
                    Some((*key, *modifiers))
                }
                _ => None,
            })
        });
        if let Some((key, mods)) = ev {
            if key == egui::Key::Escape {
                app.kb_capture = None;
                app.kb_error = None;
            } else if let Some(code) = crate::app::keybinds::encode(mods, key) {
                match crate::app::keybinds::validate(&app.settings_draft.keybindings, &action, &code)
                {
                    Ok(()) => {
                        app.settings_draft.keybindings.insert(action.clone(), code);
                        app.kb_capture = None;
                        app.kb_error = None;
                    }
                    Err(msg) => app.kb_error = Some(msg),
                }
            } else {
                app.kb_error = Some(
                    t(
                        "该键为系统保留键，或使用了不支持的组合（仅支持单键 / Ctrl+键）",
                        "Reserved key, or unsupported combo (plain keys / Ctrl+key only)",
                    )
                    .to_string(),
                );
            }
        }
    }

    dim_backdrop(ctx);
    egui::Window::new(t("设置", "Settings"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([720.0, 540.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            // Action row pinned above the scroll area — 保存/取消 never scroll away.
            // (wrapped in horizontal so the row takes one line, not the whole
            // remaining window rect, which would push the content out of view)
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(primary_button(t("保存", "Save"))).clicked() {
                        save = true;
                        app.state.show_settings = false;
                    }
                    if ui.button(t("取消", "Cancel")).clicked() {
                        // Discard draft, keep existing config.
                        app.kb_capture = None;
                        app.kb_error = None;
                        app.state.show_settings = false;
                    }
                });
            });
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                section(ui, t("常规", "General"));
                egui::Grid::new("set_general")
                    .num_columns(2)
                    .spacing([16.0, 7.0])
                    .show(ui, |ui| {
                        let d = &mut app.settings_draft;
                        ui.checkbox(&mut d.auto_open_last_workspace, t("自动打开上次工作区", "Reopen last workspace on startup"));
                        ui.end_row();
                        ui.checkbox(&mut d.auto_detect_card, t("自动检测存储卡（SD 热插拔）", "Auto-detect memory cards (SD hot-plug)"));
                        ui.end_row();
                        ui.checkbox(&mut d.dim_reviewed_thumbnails, t("缩略图淡化已阅跳过", "Dim reviewed thumbnails"));
                        ui.end_row();
                        ui.checkbox(&mut d.batch_confirm, t("批量操作二次确认", "Confirm batch operations"));
                        ui.end_row();
                        ui.checkbox(&mut d.show_clipping_warning, t("显示高光/暗部溢出提示", "Show highlight/shadow clipping hints"));
                        ui.end_row();
                        ui.checkbox(&mut d.high_dpi_2x, t("高 DPI 2x 缩略图", "High-DPI @2x thumbnails"));
                        ui.end_row();
                        ui.checkbox(&mut d.wrap_at_end, t("筛选到末尾循环跳张", "Wrap around at the end"));
                        ui.end_row();
                        ui.label(RichText::new(t("语言", "Language")).size(13.0).color(theme::TEXT_WEAK));
                        let mut lang = i18n::Lang::from_code(&d.language);
                        egui::ComboBox::from_id_salt("set_language")
                            .selected_text(lang.native_label())
                            .width(160.0)
                            .show_ui(ui, |ui| {
                                for l in [i18n::Lang::Zh, i18n::Lang::En] {
                                    ui.selectable_value(&mut lang, l, l.native_label());
                                }
                            });
                        d.language = lang.code().to_string();
                        ui.end_row();
                    });

                // 快捷键 (Shortcuts, PRD 7.2.1 / UI spec 5.3.2): click a key
                // box, press the new key; conflicts show inline and block.
                section(ui, t("快捷键", "Shortcuts"));
                ui.label(
                    RichText::new(t(
                        "点击某行的按键框后按下新键（Esc 取消）。Esc / Home / End / 数字跳片 / Ctrl+0 / Ctrl+批量键 / F11 / Ctrl+I·O 为系统保留键。",
                        "Click a key box then press the new key (Esc cancels). Esc / Home / End / digit jump / Ctrl+0 / Ctrl+batch / F11 / Ctrl+I-O are reserved.",
                    ))
                    .size(12.0)
                    .color(theme::TEXT_WEAK),
                );
                egui::Grid::new("set_keys")
                    .num_columns(2)
                    .spacing([16.0, 5.0])
                    .show(ui, |ui| {
                        for (code, zh, en) in crate::app::keybinds::ACTIONS {
                            ui.label(RichText::new(t(zh, en)).size(13.0).color(theme::TEXT));
                            let capturing = app.kb_capture.as_deref() == Some(*code);
                            let label = if capturing {
                                t("按下新键…", "Press a key…").to_string()
                            } else {
                                crate::app::keybinds::effective_codes(&app.settings_draft.keybindings, code)
                                    .iter()
                                    .map(|c| crate::app::keybinds::display(c))
                                    .collect::<Vec<_>>()
                                    .join(" / ")
                            };
                            let mut btn = egui::Button::new(RichText::new(label).size(13.0));
                            if capturing {
                                btn = btn.stroke(egui::Stroke::new(1.0, theme::ACCENT));
                            }
                            if ui.add(btn).clicked() {
                                app.kb_capture = Some((*code).to_string());
                                app.kb_error = None;
                            }
                            ui.end_row();
                        }
                    });
                if let Some(err) = &app.kb_error {
                    ui.label(RichText::new(err).size(12.0).color(theme::DELETE));
                }
                if ui.button(t("恢复默认键位", "Restore default keys")).clicked() {
                    app.settings_draft.keybindings.clear();
                    app.kb_capture = None;
                    app.kb_error = None;
                }

                section(ui, t("路径与标记", "Paths & Marks"));
                egui::Grid::new("set_paths")
                    .num_columns(2)
                    .spacing([16.0, 7.0])
                    .show(ui, |ui| {
                        let d = &mut app.settings_draft;
                        ui.label(RichText::new(t("默认目标/导出目录", "Default target/export folder")).size(13.0).color(theme::TEXT_WEAK));
                        let mut dir = d.default_target_dir.clone();
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut dir)
                                    .desired_width(340.0)
                                    .hint_text(t("留空则每询问", "Leave empty to ask every time")),
                            )
                            .changed()
                        {
                            d.default_target_dir = dir;
                        }
                        ui.end_row();
                        ui.label(RichText::new(t("XMP 星级", "XMP star rating")).size(13.0).color(theme::TEXT_WEAK));
                        let mut r = d.star_rating;
                        ui.add(egui::DragValue::new(&mut r).range(0..=5).speed(1));
                        d.star_rating = r;
                        ui.end_row();
                        ui.checkbox(
                            &mut d.export_space_guard,
                            t("导出时检测目标磁盘空间（5%+512MB）", "Check target disk space on export (5% + 512 MB)"),
                        );
                        ui.end_row();
                        ui.label(RichText::new(t("Lightroom 目录", "Lightroom folder")).size(13.0).color(theme::TEXT_WEAK));
                        ui.horizontal(|ui| {
                            let mut lr = d.lr_install_path.clone();
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut lr)
                                        .desired_width(290.0)
                                        .hint_text(t("Lightroom.exe 路径或所在目录，留空自动检测", "Path to (or folder of) Lightroom.exe; empty = auto-detect")),
                                )
                                .changed()
                            {
                                d.lr_install_path = lr;
                            }
                            if ui.button(t("浏览…", "Browse…")).clicked() {
                                if let Some(p) = rfd::FileDialog::new().pick_file() {
                                    d.lr_install_path = p.to_string_lossy().into_owned();
                                }
                            }
                        });
                        ui.end_row();
                    });

                section(ui, t("缓存", "Cache"));
                egui::Grid::new("set_cache")
                    .num_columns(2)
                    .spacing([16.0, 7.0])
                    .show(ui, |ui| {
                        let d = &mut app.settings_draft;
                        ui.label(RichText::new(t("容量上限", "Capacity limit")).size(13.0).color(theme::TEXT_WEAK));
                        let mut cap = d.cache_capacity_gb as i64;
                        if ui.add(egui::Slider::new(&mut cap, 2..=100).suffix(" GB")).changed() {
                            d.cache_capacity_gb = cap.max(2) as u64;
                        }
                        ui.end_row();
                        ui.label(RichText::new(t("过期天数", "Expire after (days)")).size(13.0).color(theme::TEXT_WEAK));
                        let mut days = d.cache_expire_days as i64;
                        if ui.add(egui::DragValue::new(&mut days).range(7..=365).suffix(t(" 天", " days"))).changed() {
                            d.cache_expire_days = days.max(7) as u64;
                        }
                        ui.end_row();
                        // 自定义缓存路径 (PRD 9.2): applied on 保存; the existing
                        // cache can then be migrated to the new root.
                        ui.label(RichText::new(t("缓存路径", "Cache folder")).size(13.0).color(theme::TEXT_WEAK));
                        ui.horizontal(|ui| {
                            let mut cd = d.cache_dir.clone();
                            if ui
                                .add(egui::TextEdit::singleline(&mut cd).desired_width(280.0))
                                .changed()
                            {
                                d.cache_dir = cd;
                            }
                            if ui.button(t("浏览…", "Browse…")).clicked() {
                                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                    d.cache_dir = p.to_string_lossy().into_owned();
                                }
                            }
                        });
                        ui.end_row();
                    });
                ui.horizontal(|ui| {
                    if let Some((bytes, files)) = app.cache_usage {
                        let msg = match i18n::lang() {
                            i18n::Lang::Zh => format!(
                                "当前缓存占用 {}（{} 个文件）",
                                crate::app::copy::human_bytes(bytes),
                                files
                            ),
                            i18n::Lang::En => format!(
                                "Cache in use {} ({} files)",
                                crate::app::copy::human_bytes(bytes),
                                files
                            ),
                        };
                        ui.label(RichText::new(msg).size(12.0).color(theme::TEXT_WEAK));
                    }
                    if app.cache_clean_running {
                        let n = app.cache_clean_progress.load(Ordering::SeqCst);
                        ui.label(
                            RichText::new(format!("{} {n}", t("清理中… 已删除", "Cleaning… deleted")))
                                .size(13.0)
                                .color(theme::ACCENT),
                        );
                    } else if ui
                        .button(t("立即清理全部过期与超限缓存", "Clean all expired & over-capacity cache now"))
                        .clicked()
                    {
                        app.start_cache_clean(usize::MAX, true);
                    }
                    if ui.button(t("打开缓存文件夹", "Open cache folder")).clicked() {
                        let _ = std::process::Command::new("explorer")
                            .arg(crate::paths::cache_dir().to_string_lossy().into_owned())
                            .spawn();
                    }
                });
                // 缓存重建 (PRD 9.6): regenerate every thumb + preview from the
                // DB on a cancellable background thread.
                ui.horizontal(|ui| {
                    if app.cache_rebuilding {
                        let done = app.cache_rebuild_done.load(Ordering::SeqCst);
                        let total = app.cache_rebuild_total.load(Ordering::SeqCst);
                        ui.label(
                            RichText::new(format!("{} ({done}/{total})", t("重建中", "Rebuilding")))
                                .size(13.0)
                                .color(theme::ACCENT),
                        );
                        if ui.button(t("取消重建", "Cancel rebuild")).clicked() {
                            app.cache_rebuild_cancel.store(true, Ordering::SeqCst);
                        }
                    } else if ui
                        .button(t("重建全部缩略图与预览图", "Rebuild all thumbnails & previews"))
                        .on_hover_text(
                            t("逐张重新解码并覆盖缩略图与预览图缓存（耗时操作）",
                              "Re-decode every photo and overwrite its thumbnail & preview caches (slow)"),
                        )
                        .clicked()
                    {
                        app.confirm = Some(ConfirmDialog {
                            title: t("重建全部缓存", "Rebuild all caches").into(),
                            text: match i18n::lang() {
                                i18n::Lang::Zh => "将遍历数据库中的全部照片，逐张重新生成并覆盖缩略图与预览图缓存。\n这是耗时操作，期间可随时取消（已重建部分保留）。\n确定开始？".to_string(),
                                i18n::Lang::En => "Every photo in the database will be re-decoded and its thumbnail & preview caches overwritten.\nThis is a slow operation and can be cancelled at any time (finished photos are kept).\nStart now?".to_string(),
                            },
                            confirm_label: t("开始重建", "Start rebuild").into(),
                            danger: false,
                            on_confirm: Box::new(|app| app.start_cache_rebuild()),
                        });
                    }
                });

                section(ui, t("数据库", "Database"));
                ui.horizontal(|ui| {
                    if ui.button(t("完整性检查", "Integrity check")).clicked() {
                        app.check_db_integrity();
                    }
                    if ui.button(t("手动备份", "Back up now")).clicked() {
                        app.manual_db_backup();
                    }
                    if ui.button(t("恢复备份…", "Restore backup…")).clicked() {
                        restore_clicked = true;
                    }
                });
                ui.label(
                    RichText::new(t(
                        "手动备份会保存一份当前数据库快照（保留最近 5 份）；恢复备份会用所选备份替换当前库。",
                        "Back up now saves a snapshot of the current database (keeps the latest 5); restoring replaces the current database with the chosen backup.",
                    ))
                    .size(12.0)
                    .color(theme::TEXT_WEAK),
                );

                section(ui, t("关于", "About"));
                ui.label(
                    RichText::new(format!("咔咔 Kaka v{}", env!("CARGO_PKG_VERSION")))
                        .size(14.0)
                        .color(theme::TEXT),
                );
                ui.label(
                    RichText::new(t("开源 · 无自动更新 · Windows x86_64", "Open source · no auto-update · Windows x86_64"))
                        .size(12.0)
                        .color(theme::TEXT_WEAK),
                );
                ui.horizontal(|ui| {
                    if ui.button(t("打开 GitHub 仓库", "Open GitHub repository")).clicked() {
                        open_url(&app.state.config.github_repo);
                    }
                    if ui.button(t("打开日志文件夹", "Open log folder")).clicked() {
                        let _ = std::process::Command::new("explorer")
                            .arg(crate::paths::logs_dir().to_string_lossy().into_owned())
                            .spawn();
                    }
                });
            });
        });
    if restore_clicked {
        let start_dir = app
            .state
            .db
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        if let Some(p) = rfd::FileDialog::new()
            .set_title(t("选择备份文件", "Select backup file"))
            .add_filter("SQLite 备份", &["bak"])
            .set_directory(start_dir)
            .pick_file()
        {
            app.restore_db_pick(&p);
        }
    }
    if save {
        // Apply draft to live config + persist + switch the UI language.
        app.kb_capture = None;
        app.kb_error = None;

        // 缓存路径变更 (PRD 9.2): validate the new path, reroute all cache IO
        // via the override, and offer to migrate the existing cache across.
        let old_cache = app.state.config.cache_dir.clone();
        let mut new_cache = app.settings_draft.cache_dir.trim().to_string();
        if new_cache.is_empty() {
            new_cache = crate::paths::default_cache_dir().to_string_lossy().into_owned();
        }
        if new_cache != old_cache {
            match std::fs::create_dir_all(&new_cache) {
                Ok(()) => {
                    crate::paths::set_cache_override(Some(std::path::PathBuf::from(&new_cache)));
                    app.settings_draft.cache_dir = new_cache.clone();
                    if std::path::Path::new(&old_cache).is_dir() {
                        let old_path = std::path::PathBuf::from(&old_cache);
                        let new_path = std::path::PathBuf::from(&new_cache);
                        app.confirm = Some(ConfirmDialog {
                            title: t("迁移缓存", "Migrate cache").into(),
                            text: match i18n::lang() {
                                i18n::Lang::Zh => format!(
                                    "检测到缓存路径变更。\n确认：把旧缓存（{old_cache}）迁移到新路径并删除旧目录。\n取消：保留旧缓存目录，之后可手动删除。"
                                ),
                                i18n::Lang::En => format!(
                                    "Cache path changed.\nConfirm: migrate the old cache ({old_cache}) to the new path and delete the old folder.\nCancel: keep the old cache folder (delete it manually later)."
                                ),
                            },
                            confirm_label: t("迁移并删除旧缓存", "Migrate & delete old").into(),
                            danger: false,
                            on_confirm: Box::new(move |app| {
                                app.start_cache_migration(old_path, new_path);
                            }),
                        });
                    }
                }
                Err(e) => {
                    app.toast(
                        ToastKind::Error,
                        format!("{}{e}", t("缓存路径无法创建，已保留原路径：", "Cannot create cache folder, keeping the old path: ")),
                    );
                    app.settings_draft.cache_dir = old_cache;
                }
            }
        }

        app.state.config = app.settings_draft.clone();
        i18n::set_lang(i18n::Lang::from_code(&app.state.config.language));
        if let Err(e) = crate::config::save(&app.state.config) {
            app.toast(ToastKind::Error, format!("{}{e}", t("设置保存失败：", "Failed to save settings: ")));
        } else {
            app.toast(ToastKind::Success, t("设置已保存", "Settings saved"));
        }
    }
}

/// A settings section heading with a little breathing room.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).size(13.0).color(theme::ACCENT).strong());
    ui.separator();
}

/// Open a URL / folder in the system handler (Windows).
fn open_url(url: &str) {
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .spawn();
}

fn filter_dialog(app: &mut KakaApp, ctx: &egui::Context) {
    let folder = app.state.ws.folder_path.clone();
    let cameras = db::photos::distinct_camera_models(&app.state.db, &folder).unwrap_or_default();
    let lenses = db::photos::distinct_lens_models(&app.state.db, &folder).unwrap_or_default();
    let formats = common_formats(&app.state.ws.items);

    dim_backdrop(ctx);
    egui::Window::new("高级过滤")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([620.0, 560.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            ui.label(RichText::new(t("高级过滤（可叠加，条件为 AND 关系）", "Advanced filter (stackable, all AND)")).heading().color(theme::TEXT));
            ui.add_space(6.0);

            let mut apply = false;
            let mut clear = false;
            let mut cancel = false;

            egui::ScrollArea::vertical().show(ui, |ui| {
                // 状态.
                section_heading(ui, t("状态", "Status"));
                ui.horizontal(|ui| {
                    for (id, label) in [(0i64, t("未处理", "Untreated")), (1, t("待删", "To delete")), (2, t("已阅", "Reviewed"))] {
                        let mut on = app.filter_draft.statuses.contains(&id);
                        if ui.checkbox(&mut on, label).changed() {
                            toggle_in(&mut app.filter_draft.statuses, id);
                        }
                    }
                    if ui.button(t("全部", "All")).clicked() {
                        app.filter_draft.statuses.clear();
                    }
                });

                // 相机 / 镜头.
                section_heading(ui, t("相机型号", "Camera model"));
                ui.horizontal_wrapped(|ui| {
                    if cameras.is_empty() {
                        ui.label(RichText::new(t("（无）", "(none)")).color(theme::TEXT_WEAK));
                    }
                    for cam in &cameras {
                        let mut on = app.filter_draft.cameras.contains(cam);
                        if ui.checkbox(&mut on, cam).changed() {
                            toggle_str(&mut app.filter_draft.cameras, cam);
                        }
                    }
                });
                section_heading(ui, t("镜头型号", "Lens model"));
                ui.horizontal_wrapped(|ui| {
                    if lenses.is_empty() {
                        ui.label(RichText::new(t("（无）", "(none)")).color(theme::TEXT_WEAK));
                    }
                    for lens in &lenses {
                        let mut on = app.filter_draft.lenses.contains(lens);
                        if ui.checkbox(&mut on, lens).changed() {
                            toggle_str(&mut app.filter_draft.lenses, lens);
                        }
                    }
                });

                // ISO / 焦距.
                section_heading(ui, t("ISO 范围", "ISO range"));
                ui.horizontal(|ui| {
                    let mut mn = app.filter_draft.iso_min.unwrap_or(0);
                    let mut mx = app.filter_draft.iso_max.unwrap_or(0);
                    ui.label(RichText::new("min").color(theme::TEXT_WEAK));
                    if ui.add(egui::DragValue::new(&mut mn).range(0..=102400).speed(50)).changed() {
                        app.filter_draft.iso_min = Some(mn);
                    }
                    ui.label(RichText::new("max").color(theme::TEXT_WEAK));
                    if ui.add(egui::DragValue::new(&mut mx).range(0..=102400).speed(50)).changed() {
                        app.filter_draft.iso_max = Some(mx);
                    }
                    if ui.button(t("清除", "Clear")).clicked() {
                        app.filter_draft.iso_min = None;
                        app.filter_draft.iso_max = None;
                    }
                });
                section_heading(ui, t("焦距范围 (mm)", "Focal range (mm)"));
                ui.horizontal(|ui| {
                    let mut mn = app.filter_draft.focal_min.unwrap_or(0);
                    let mut mx = app.filter_draft.focal_max.unwrap_or(0);
                    ui.label(RichText::new("min").color(theme::TEXT_WEAK));
                    if ui.add(egui::DragValue::new(&mut mn).range(0..=2000).speed(1)).changed() {
                        app.filter_draft.focal_min = Some(mn);
                    }
                    ui.label(RichText::new("max").color(theme::TEXT_WEAK));
                    if ui.add(egui::DragValue::new(&mut mx).range(0..=2000).speed(1)).changed() {
                        app.filter_draft.focal_max = Some(mx);
                    }
                    if ui.button(t("清除", "Clear")).clicked() {
                        app.filter_draft.focal_min = None;
                        app.filter_draft.focal_max = None;
                    }
                });

                // 光圈范围 (PRD 7.8): numeric f-value min/max.
                section_heading(ui, t("光圈范围 (f 值)", "Aperture range (f)"));
                ui.horizontal(|ui| {
                    let mut mn = app.filter_draft.aperture_min.unwrap_or(1.0);
                    let mut mx = app.filter_draft.aperture_max.unwrap_or(32.0);
                    ui.label(RichText::new("min").color(theme::TEXT_WEAK));
                    if ui
                        .add(egui::DragValue::new(&mut mn).range(0.1..=128.0).speed(0.1).suffix(" f"))
                        .changed()
                    {
                        app.filter_draft.aperture_min = Some(mn);
                    }
                    ui.label(RichText::new("max").color(theme::TEXT_WEAK));
                    if ui
                        .add(egui::DragValue::new(&mut mx).range(0.1..=128.0).speed(0.1).suffix(" f"))
                        .changed()
                    {
                        app.filter_draft.aperture_max = Some(mx);
                    }
                    if ui.button(t("清除", "Clear")).clicked() {
                        app.filter_draft.aperture_min = None;
                        app.filter_draft.aperture_max = None;
                    }
                });

                // 快门范围 (PRD 7.8): 最快/最慢，秒数手输 + 预设档位 1/1000…1s。
                section_heading(ui, t("快门范围", "Shutter range"));
                ui.horizontal(|ui| {
                    // Preset stops from 1/1000s to 1s (seconds).
                    const PRESETS: [f64; 11] = [
                        0.001, 0.002, 0.004, 0.008, 0.016667, 0.033333, 0.066667, 0.125, 0.25,
                        0.5, 1.0,
                    ];
                    let label = |sec: f64| -> String {
                        if sec >= 1.0 {
                            format!("{}s", sec)
                        } else {
                            format!("1/{}s", (1.0 / sec).round() as i64)
                        }
                    };
                    ui.label(RichText::new(t("最快", "Fastest")).size(12.0).color(theme::TEXT_WEAK));
                    egui::ComboBox::from_id_salt("shutter_min_preset")
                        .width(90.0)
                        .selected_text(
                            app.filter_draft
                                .shutter_min
                                .map(|v| label(v))
                                .unwrap_or_else(|| t("预设", "Preset").to_string()),
                        )
                        .show_ui(ui, |ui| {
                            for p in PRESETS {
                                ui.selectable_value(
                                    &mut app.filter_draft.shutter_min,
                                    Some(p),
                                    label(p),
                                );
                            }
                        });
                    let mut mn = app.filter_draft.shutter_min.unwrap_or(0.001);
                    if ui
                        .add(egui::DragValue::new(&mut mn).range(0.0001..=60.0).speed(0.05).suffix("s"))
                        .changed()
                    {
                        app.filter_draft.shutter_min = Some(mn);
                    }
                    ui.separator();
                    ui.label(RichText::new(t("最慢", "Slowest")).size(12.0).color(theme::TEXT_WEAK));
                    egui::ComboBox::from_id_salt("shutter_max_preset")
                        .width(90.0)
                        .selected_text(
                            app.filter_draft
                                .shutter_max
                                .map(|v| label(v))
                                .unwrap_or_else(|| t("预设", "Preset").to_string()),
                        )
                        .show_ui(ui, |ui| {
                            for p in PRESETS {
                                ui.selectable_value(
                                    &mut app.filter_draft.shutter_max,
                                    Some(p),
                                    label(p),
                                );
                            }
                        });
                    let mut mx = app.filter_draft.shutter_max.unwrap_or(30.0);
                    if ui
                        .add(egui::DragValue::new(&mut mx).range(0.0001..=60.0).speed(0.05).suffix("s"))
                        .changed()
                    {
                        app.filter_draft.shutter_max = Some(mx);
                    }
                    if ui.button(t("清除", "Clear")).clicked() {
                        app.filter_draft.shutter_min = None;
                        app.filter_draft.shutter_max = None;
                    }
                });

                // 日期范围.
                section_heading(ui, t("拍摄日期范围", "Capture date range"));
                ui.horizontal(|ui| {
                    let mut df = app.filter_draft.date_from.clone().unwrap_or_default();
                    let mut dt = app.filter_draft.date_to.clone().unwrap_or_default();
                    ui.label(RichText::new(t("从", "From")).color(theme::TEXT_WEAK));
                    if ui.add(egui::TextEdit::singleline(&mut df).hint_text("YYYY-MM-DD").desired_width(110.0)).changed() {
                        app.filter_draft.date_from = if df.trim().is_empty() { None } else { Some(df.trim().to_string()) };
                    }
                    ui.label(RichText::new(t("到", "To")).color(theme::TEXT_WEAK));
                    if ui.add(egui::TextEdit::singleline(&mut dt).hint_text("YYYY-MM-DD").desired_width(110.0)).changed() {
                        app.filter_draft.date_to = if dt.trim().is_empty() { None } else { Some(dt.trim().to_string()) };
                    }
                    quick_date(ui, &mut app.filter_draft);
                });

                // 文件格式.
                section_heading(ui, t("文件格式", "File format"));
                ui.horizontal_wrapped(|ui| {
                    for f in &formats {
                        let mut on = app.filter_draft.formats.contains(f);
                        if ui.checkbox(&mut on, f).changed() {
                            toggle_str(&mut app.filter_draft.formats, f);
                        }
                    }
                });

                // 是否丢失 / 配对.
                section_heading(ui, t("文件状态", "File state"));
                ui.horizontal(|ui| {
                    let mut missing = app.filter_draft.missing;
                    radio_opt(ui, &mut missing, None, t("全部", "All"));
                    radio_opt(ui, &mut missing, Some(true), t("仅丢失", "Missing only"));
                    radio_opt(ui, &mut missing, Some(false), t("不包含丢失", "Existing only"));
                    app.filter_draft.missing = missing;
                });
                ui.horizontal(|ui| {
                    let mut pair = app.filter_draft.pair;
                    radio_opt(ui, &mut pair, None, t("全部", "All"));
                    radio_opt(ui, &mut pair, Some(true), t("仅配对", "Paired only"));
                    radio_opt(ui, &mut pair, Some(false), t("仅单文件", "Singles only"));
                    app.filter_draft.pair = pair;
                });
            });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.add(primary_button(t("应用", "Apply"))).clicked() {
                    apply = true;
                }
                if ui.button(t("清除过滤", "Clear filters")).clicked() {
                    clear = true;
                }
                if ui.button(t("取消", "Cancel")).clicked() {
                    cancel = true;
                }
            });
            if apply {
                app.state.ws.filter = app.filter_draft.clone();
                let _ = app.state.reload_current();
                app.state.show_filter = false;
            }
            if clear {
                app.filter_draft = crate::model::Filter::default();
                app.state.ws.filter = crate::model::Filter::default();
                let _ = app.state.reload_current();
                app.state.show_filter = false;
                app.toast(ToastKind::Info, t("已清除过滤条件", "Filters cleared"));
            }
            if cancel {
                app.state.show_filter = false;
            }
        });
    ctx.request_repaint();
}

fn section_heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).size(13.0).color(theme::ACCENT).strong());
}

fn toggle_in(vec: &mut Vec<i64>, v: i64) {
    if let Some(pos) = vec.iter().position(|x| *x == v) {
        vec.remove(pos);
    } else {
        vec.push(v);
    }
}

fn toggle_str(vec: &mut Vec<String>, v: &str) {
    if let Some(pos) = vec.iter().position(|x| x == v) {
        vec.remove(pos);
    } else {
        vec.push(v.to_string());
    }
}

fn radio_opt(ui: &mut egui::Ui, target: &mut Option<bool>, v: Option<bool>, label: &str) {
    ui.radio_value(target, v, label);
}

fn quick_date(ui: &mut egui::Ui, f: &mut crate::model::Filter) {
    let today = chrono::Local::now().date_naive();
    let month_start = chrono::NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap_or(today);
    let prev_month_end = month_start - chrono::Duration::days(1);
    let prev_month_start = chrono::NaiveDate::from_ymd_opt(prev_month_end.year(), prev_month_end.month(), 1)
        .unwrap_or(prev_month_end);
    let mut push = |ui: &mut egui::Ui, label: &str, from: chrono::NaiveDate, to: chrono::NaiveDate| {
        if ui.button(label).clicked() {
            f.date_from = Some(from.format("%Y-%m-%d").to_string());
            f.date_to = Some(to.format("%Y-%m-%d").to_string());
        }
    };
    push(ui, t("今日", "Today"), today, today);
    push(ui, t("昨日", "Yesterday"), today - chrono::Duration::days(1), today - chrono::Duration::days(1));
    push(ui, t("近7天", "Last 7 days"), today - chrono::Duration::days(6), today);
    push(ui, t("本月", "This month"), month_start, today);
    push(ui, t("上月", "Last month"), prev_month_start, prev_month_end);
}

fn common_formats(items: &[crate::model::PhotoListItem]) -> Vec<String> {
    let mut set: Vec<String> = Vec::new();
    for p in items {
        if let Some(ext) = std::path::Path::new(&p.original_filename)
            .extension()
            .and_then(|e| e.to_str())
        {
            let ext = ext.to_uppercase();
            if !set.contains(&ext) {
                set.push(ext);
            }
        }
    }
    set
}

fn export_dialog(app: &mut KakaApp, ctx: &egui::Context) {
    let folder = app.state.ws.folder_path.clone();
    let mut copy_clicked = false;
    let mut list_clicked = false;
    let mut xmp_clicked = false;
    let mut lr_send = false;

    dim_backdrop(ctx);
    egui::Window::new(t("导出", "Export"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([620.0, 460.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            // 标题栏已显示「导出」，内容区不再重复标题（与设置面板同例）。
            ui.label(RichText::new(t("仅导出「保留」照片（未标记待删），不改动源文件与数据库。", "Exports only kept photos (not marked for deletion); never touches source files or the database."))
                .size(13.0).color(theme::TEXT_SECONDARY));
            ui.separator();

            // 12.1 复制保留照片到目录.
            ui.label(RichText::new(t("方式一：复制保留照片到指定目录", "1. Copy kept photos to a folder")).size(14.0).color(theme::ACCENT).strong());
            ui.horizontal(|ui| {
                let mut target = app.export_target.clone();
                if ui.add(egui::TextEdit::singleline(&mut target).desired_width(430.0).hint_text("目标导出目录")).changed() {
                    app.export_target = target;
                }
                if ui.button(t("浏览…", "Browse…")).clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        app.export_target = p.to_string_lossy().into_owned();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(t("组织方式", "Layout")).size(12.0).color(theme::TEXT_WEAK));
                ui.radio_value(&mut app.export_org, crate::app::copy::OrgMode::Structure, t("保持原结构", "Keep original structure"));
                ui.radio_value(&mut app.export_org, crate::app::copy::OrgMode::Date, t("按拍摄日期", "By capture date"));
                ui.radio_value(&mut app.export_org, crate::app::copy::OrgMode::Flat, t("全部平铺", "Flat"));
            });
            if app.export_copy_running {
                // 12.1 后台导出：进度 + 当前文件名 + 取消（UI 不冻结）。
                let done = app.export_copy_progress.done.load(Ordering::SeqCst);
                let total = app.export_copy_progress.total.load(Ordering::SeqCst);
                let current = app
                    .export_copy_progress
                    .current
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new(format!("{} ({done}/{total})", t("导出中", "Exporting")))
                            .size(13.0)
                            .color(theme::ACCENT),
                    );
                    if ui.button(t("取消", "Cancel")).clicked() {
                        app.export_copy_cancel.store(true, Ordering::SeqCst);
                    }
                });
                ui.label(
                    RichText::new(current).size(12.0).color(theme::TEXT_WEAK),
                );
            } else if ui.button(t("开始导出复制", "Start copy export")).clicked() {
                copy_clicked = true;
            }
            // 已完成一次导出：结果摘要 + 失败列表（保留到下次导出）。
            if let Some(Ok(report)) = app.export_copy_result.clone() {
                let summary = if report.cancelled {
                    match i18n::lang() {
                        i18n::Lang::Zh => format!("已取消：成功 {} 张 / 未完成 {} 张", report.copied, report.failed),
                        i18n::Lang::En => format!("Cancelled: {} copied, {} unfinished", report.copied, report.failed),
                    }
                } else {
                    match i18n::lang() {
                        i18n::Lang::Zh => format!("结果：成功 {} 张 / 失败 {} 张", report.copied, report.failed),
                        i18n::Lang::En => format!("Result: {} copied, {} failed", report.copied, report.failed),
                    }
                };
                ui.label(RichText::new(summary).size(12.0).color(theme::TEXT_SECONDARY));
                if !report.failures.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "{}（{}/{}）",
                            t("失败列表", "Failed files"),
                            report.failures.len(),
                            report.total
                        ))
                        .size(12.0)
                        .color(theme::DELETE),
                    );
                    failures_table(ui, &report.failures, true);
                    ui.horizontal(|ui| {
                        if ui
                            .button(t("导出失败列表 .csv", "Export failures .csv"))
                            .clicked()
                        {
                            if let Some(p) = rfd::FileDialog::new()
                                .add_filter("CSV", &["csv"])
                                .set_file_name("导出失败列表.csv")
                                .save_file()
                            {
                                match crate::app::import::export_failure_csv(
                                    &p.to_string_lossy(),
                                    &report.failures,
                                ) {
                                    Ok(n) => app.toast(
                                        ToastKind::Success,
                                        format!("{}{n} {}", t("已导出 ", "Exported "), t("条失败记录", "failure records")),
                                    ),
                                    Err(e) => app.toast(
                                        ToastKind::Error,
                                        format!("{}{e}", t("导出失败：", "Export failed: ")),
                                    ),
                                }
                            }
                        }
                    });
                }
            } else if let Some(Err(e)) = &app.export_copy_result {
                ui.label(
                    RichText::new(format!("{}{e}", t("导出失败：", "Export failed: ")))
                        .size(12.0)
                        .color(theme::DELETE),
                );
            }

            ui.add_space(8.0);
            ui.separator();
            ui.label(RichText::new(t("方式二：生成保留照片文件列表", "2. Export a kept-photo file list")).size(14.0).color(theme::ACCENT).strong());
            if ui.button(t("导出 .txt / .csv 列表", "Export .txt / .csv list")).clicked() {
                list_clicked = true;
            }

            ui.add_space(8.0);
            ui.separator();
            ui.label(RichText::new(t(
                "方式三：写入 XMP 标记（Kaka:Keep + 星级；RAW 写侧车，JPEG/PNG 额外内嵌进文件——LR 不读非 RAW 的侧车）",
                "3. Write XMP marks (Kaka:Keep + rating; RAW=sidecar, JPEG/PNG also embedded — LR ignores non-RAW sidecars)",
            )).size(14.0).color(theme::ACCENT).strong());
            if ui.button(t("写入 XMP 标记", "Write XMP marks")).clicked() {
                xmp_clicked = true;
            }

            ui.add_space(8.0);
            ui.separator();
            ui.label(RichText::new(t("方式四：发送到 Lightroom 经典版", "4. Send to Lightroom Classic")).size(14.0).color(theme::ACCENT).strong());
            let lr_found = app.lr_path.is_some();
            let send_btn = egui::Button::new(RichText::new(t("发送到 Lightroom", "Send to Lightroom")).color(theme::TEXT));
            if ui.add_enabled(lr_found, send_btn)
                .on_hover_text(if lr_found { t("将保留照片发送给 Lightroom 导入", "Send kept photos to Lightroom for import") } else { t("未检测到 Lightroom 经典版", "Lightroom Classic not detected") })
                .clicked()
            {
                lr_send = true;
            }

            ui.separator();
            if ui.button(t("关闭", "Close")).clicked() {
                app.state.show_export = false;
            }
        });
    ctx.request_repaint();

    if copy_clicked {
        let target = app.export_target.trim().to_string();
        if target.is_empty() {
            app.toast(ToastKind::Warning, t("请先选择导出目录", "Pick an export folder first"));
        } else {
            // 12.1: run on a background thread — the dialog stays open and
            // shows progress; the result (with failure list) is reported
            // there and via toast when finished.
            app.start_export_copy(folder.clone(), target);
        }
    }
    if list_clicked {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("文本列表", &["txt"])
            .add_filter("CSV", &["csv"])
            .set_file_name("保留照片列表.csv")
            .save_file()
        {
            let format = if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("txt")).unwrap_or(false) {
                crate::app::export::ExportFileFormat::Txt
            } else {
                crate::app::export::ExportFileFormat::Csv
            };
            match crate::app::export::export_file_list(&app.state.db, &folder, &path.to_string_lossy(), format) {
                Ok(n) => {
                    let msg = match i18n::lang() {
                        i18n::Lang::Zh => format!("已导出 {n} 张列表到：{}", path.display()),
                        i18n::Lang::En => format!("Exported {n} entries to: {}", path.display()),
                    };
                    app.toast(ToastKind::Success, msg)
                }
                Err(e) => app.toast(ToastKind::Error, format!("{}{e}", t("导出列表失败：", "List export failed: "))),
            }
            app.state.show_export = false;
        }
    }
    if xmp_clicked {
        let rating = app.state.config.star_rating;
        match crate::app::export::write_xmp_sidecars(&app.state.db, &folder, rating) {
            Ok(n) => {
                    let msg = match i18n::lang() {
                        i18n::Lang::Zh => format!("已为 {n} 张保留照片写入 XMP 标记"),
                        i18n::Lang::En => format!("Wrote XMP marks for {n} kept photos"),
                    };
                    app.toast(ToastKind::Success, msg)
                }
            Err(e) => app.toast(ToastKind::Error, format!("{}{e}", t("写入 XMP 失败：", "XMP write failed: "))),
        }
        app.state.show_export = false;
    }
    if lr_send {
        if let Some(exe) = app.lr_path.clone() {
            match crate::app::export::send_to_lightroom(&app.state.db, &folder, &exe) {
                Ok(n) => {
                    let msg = match i18n::lang() {
                        i18n::Lang::Zh => format!("已发送 {n} 张保留照片到 Lightroom"),
                        i18n::Lang::En => format!("Sent {n} kept photos to Lightroom"),
                    };
                    app.toast(ToastKind::Success, msg)
                }
                Err(e) => app.toast(ToastKind::Error, format!("{}{e}", t("发送到 Lightroom 失败：", "Send to Lightroom failed: "))),
            }
            app.state.show_export = false;
        }
    }
}

/// PRD 6.5 第三步 去重扫描 + UI 5.1-4 文件网格: pre-scan status line, stats
/// bar, toolbar (sort / filter / zoom / select) and the checkable grid.
fn import_scan_area(app: &mut KakaApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    use crate::app::import::PrescanMark;
    use crate::app::ui::app::{ImportScanFilter, ImportScanSort};

    // (Re)scan trigger: source path or recursive switch changed. Typed edits
    // are debounced ~0.5s so each keystroke doesn't restart the scan; the
    // recursive checkbox and 重新扫描 fire immediately (dirty_since in past).
    let now = ctx.input(|i| i.time);
    let key_now = (app.import_path.trim().to_string(), app.import_recursive);
    if app.import_scan_key.as_ref() == Some(&key_now) {
        app.import_scan_dirty_since = None;
    } else if !app.state.import_running {
        let since = *app.import_scan_dirty_since.get_or_insert(now);
        if now - since >= 0.5 && !app.import_scan_running {
            app.import_scan_dirty_since = None;
            let valid = std::path::Path::new(&key_now.0).is_dir();
            app.import_scan_key = Some(key_now.clone());
            if valid {
                app.start_import_prescan(key_now.0.clone(), key_now.1);
            } else {
                app.import_scan_files.clear();
                app.import_scan_selected.clear();
            }
        }
    }

    if app.import_scan_running {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(18.0));
            let d = app.import_scan_done.load(Ordering::SeqCst);
            let tot = app.import_scan_total.load(Ordering::SeqCst);
            let text = if tot > 0 {
                format!("{} ({d}/{tot})", t("正在扫描与比对…", "Scanning & comparing…"))
            } else {
                t("正在扫描…", "Scanning…").to_string()
            };
            ui.label(RichText::new(text).size(13.0).color(theme::ACCENT));
            if ui.button(t("取消扫描", "Cancel scan")).clicked() {
                app.import_scan_cancel.store(true, Ordering::SeqCst);
            }
        });
        return;
    }
    if app.import_scan_files.is_empty() {
        ui.horizontal(|ui| {
            let path_ok = std::path::Path::new(app.import_path.trim()).is_dir();
            let hint = if app.import_path.trim().is_empty() {
                t("选择源路径后将自动扫描并列出待导入文件。", "Pick a source folder — files are listed here automatically.")
            } else if !path_ok {
                t("源路径不存在或不是文件夹。", "Source path does not exist or is not a folder.")
            } else {
                t("扫描已取消，可重新扫描。", "Scan cancelled — rescan anytime.")
            };
            ui.label(RichText::new(hint).size(12.0).color(theme::TEXT_WEAK));
            if path_ok && ui.button(t("重新扫描", "Rescan")).clicked() {
                app.import_scan_key = None;
                app.import_scan_dirty_since = None;
            }
        });
        return;
    }

    // 统计条：源共 N 张，已存在 Y 张，将导入 X 张，RAW+JPG M 组 · 预计大小 S。
    let n = app.import_scan_files.len();
    let exists_n = app
        .import_scan_files
        .iter()
        .filter(|f| f.mark != PrescanMark::New)
        .count();
    let to_import_n = app
        .import_scan_files
        .iter()
        .filter(|f| f.mark == PrescanMark::New && app.import_scan_selected.contains(&f.path))
        .count();
    let pairs = app
        .import_scan_files
        .iter()
        .filter_map(|f| f.pair_group)
        .collect::<std::collections::HashSet<_>>()
        .len();
    let size_bytes: i64 = app
        .import_scan_files
        .iter()
        .filter(|f| f.mark == PrescanMark::New && app.import_scan_selected.contains(&f.path))
        .map(|f| f.file_size)
        .sum();
    let stats = match i18n::lang() {
        i18n::Lang::Zh => format!(
            "源共 {n} 张，已存在 {exists_n} 张，将导入 {to_import_n} 张，RAW+JPG {pairs} 组 · 预计大小 {}",
            crate::app::copy::human_bytes(size_bytes)
        ),
        i18n::Lang::En => format!(
            "{n} files, {exists_n} in library, {to_import_n} to import, {pairs} RAW+JPG pairs · ~{}",
            crate::app::copy::human_bytes(size_bytes)
        ),
    };
    ui.label(RichText::new(stats).size(12.5).color(theme::TEXT_SECONDARY));

    // 工具栏：排序 / 筛选 / 视图缩放 / 全选反选。
    let copy_mode = app.import_mode == crate::app::state::ImportMode::Copy;
    let actionable =
        |f: &crate::app::import::PrescanItem| -> bool {
            f.mark == PrescanMark::New || (copy_mode && f.mark == PrescanMark::Exists)
        };
    ui.horizontal(|ui| {
        ui.label(RichText::new(t("排序", "Sort")).size(12.0).color(theme::TEXT_WEAK));
        egui::ComboBox::from_id_salt("import_scan_sort")
            .selected_text(match app.import_scan_sort {
                ImportScanSort::CaptureTime => t("拍摄时间", "Capture time"),
                ImportScanSort::Filename => t("文件名", "Filename"),
                ImportScanSort::Size => t("大小", "Size"),
            })
            .width(92.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.import_scan_sort, ImportScanSort::CaptureTime, t("拍摄时间", "Capture time"));
                ui.selectable_value(&mut app.import_scan_sort, ImportScanSort::Filename, t("文件名", "Filename"));
                ui.selectable_value(&mut app.import_scan_sort, ImportScanSort::Size, t("大小", "Size"));
            });
        ui.separator();
        if ui
            .selectable_label(app.import_scan_filter == ImportScanFilter::All, t("全部", "All"))
            .clicked()
        {
            app.import_scan_filter = ImportScanFilter::All;
        }
        if ui
            .selectable_label(app.import_scan_filter == ImportScanFilter::ToImport, t("仅待导入", "To import"))
            .clicked()
        {
            app.import_scan_filter = ImportScanFilter::ToImport;
        }
        if ui
            .selectable_label(app.import_scan_filter == ImportScanFilter::Exists, t("仅已存在", "Existing"))
            .clicked()
        {
            app.import_scan_filter = ImportScanFilter::Exists;
        }
        ui.separator();
        ui.label(RichText::new(t("视图", "Zoom")).size(12.0).color(theme::TEXT_WEAK));
        let cell_label = match app.import_scan_cell {
            0 => "S",
            2 => "L",
            _ => "M",
        };
        ui.add_sized(
            [80.0, 20.0],
            egui::Slider::new(&mut app.import_scan_cell, 0..=2)
                .show_value(false)
                .text(cell_label),
        );
        ui.separator();
        if ui
            .button(t("全选", "Select all"))
            .on_hover_text(t("仅作用于可勾选项", "Checkable items only"))
            .clicked()
        {
            for f in &app.import_scan_files {
                if actionable(f) {
                    app.import_scan_selected.insert(f.path.clone());
                }
            }
        }
        if ui
            .button(t("反选", "Invert"))
            .on_hover_text(t("仅作用于可勾选项", "Checkable items only"))
            .clicked()
        {
            let mut to_remove = Vec::new();
            for f in &app.import_scan_files {
                if actionable(f) {
                    if app.import_scan_selected.contains(&f.path) {
                        to_remove.push(f.path.clone());
                    } else {
                        app.import_scan_selected.insert(f.path.clone());
                    }
                }
            }
            for p in to_remove {
                app.import_scan_selected.remove(&p);
            }
        }
    });

    // 文件网格：宽度自适应每行 4-10 张。
    let cell = crate::app::ui::app::IMPORT_SCAN_CELL_SIZES[app.import_scan_cell];
    let cols = (((ui.available_width() - 14.0) / (cell + 8.0)).floor() as usize).clamp(4, 10);
    let mut order: Vec<usize> = (0..app.import_scan_files.len()).collect();
    match app.import_scan_sort {
        ImportScanSort::CaptureTime => order.sort_by(|&a, &b| {
            app.import_scan_files[a]
                .capture_time
                .cmp(&app.import_scan_files[b].capture_time)
                .then_with(|| app.import_scan_files[a].filename.cmp(&app.import_scan_files[b].filename))
        }),
        ImportScanSort::Filename => {
            order.sort_by(|&a, &b| app.import_scan_files[a].filename.cmp(&app.import_scan_files[b].filename))
        }
        ImportScanSort::Size => {
            order.sort_by(|&a, &b| app.import_scan_files[b].file_size.cmp(&app.import_scan_files[a].file_size))
        }
    }
    order.retain(|&i| match app.import_scan_filter {
        ImportScanFilter::All => true,
        ImportScanFilter::ToImport => app.import_scan_files[i].mark == PrescanMark::New,
        ImportScanFilter::Exists => app.import_scan_files[i].mark != PrescanMark::New,
    });
    // 为底部主按钮行预留空间（egui 坑：占满 available 会把按钮挤出窗口）。
    let grid_h = (ui.available_height() - 70.0).max(140.0);
    // DEBUG（抖动诊断）：布局签名变化时记日志，静止时若仍刷屏即为逐帧振荡。
    let dbg_mr = ui.max_rect();
    let dbg_sig = (
        cols,
        (grid_h * 2.0).round() as i32,
        (dbg_mr.width() * 2.0).round() as i32,
        (dbg_mr.height() * 2.0).round() as i32,
        (ui.available_width() * 2.0).round() as i32,
    );
    if dbg_sig != app.import_scan_dbg_sig {
        let (t, sr) = ctx.input(|i| (i.time, i.viewport_rect()));
        log::info!(
            "import grid layout: t={t:.4} cols={cols} grid_h={grid_h:.1} max_rect={dbg_mr:?} avail_w={:.1} screen={sr:?}",
            ui.available_width()
        );
        app.import_scan_dbg_sig = dbg_sig;
    }
    // S/M/L 切换时保持滚动位置比例（内容高度骤变会让视图大幅跳变）。
    let cell_changed = app.import_scan_last_cell != app.import_scan_cell;
    let mut scroll = egui::ScrollArea::vertical()
        .max_height(grid_h)
        .auto_shrink([false, false]);
    if cell_changed {
        let ratio = (cell + 6.0)
            / (crate::app::ui::app::IMPORT_SCAN_CELL_SIZES[app.import_scan_last_cell] + 6.0);
        scroll = scroll.vertical_scroll_offset(app.import_scan_scroll_offset * ratio);
        app.import_scan_last_cell = app.import_scan_cell;
    }
    let out = scroll.show(ui, |ui| {
        egui::Grid::new("import_scan_grid")
            .spacing([6.0, 6.0])
            .show(ui, |ui| {
                for chunk in order.chunks(cols) {
                    for &i in chunk {
                        import_scan_cell(app, ui, i, cell, copy_mode);
                    }
                    ui.end_row();
                }
            });
    });
    app.import_scan_scroll_offset = out.state.offset.y;
}

/// One cell of the import pre-scan grid: 140x140-style frame + thumbnail
/// (Spinner placeholder while the background job runs), checkbox overlay,
/// grayed + bottom label for 已存在, hover accent border.
fn import_scan_cell(
    app: &mut KakaApp,
    ui: &mut egui::Ui,
    idx: usize,
    cell: f32,
    copy_mode: bool,
) {
    use crate::app::import::PrescanMark;
    let item = &app.import_scan_files[idx];
    let selected = app.import_scan_selected.contains(&item.path);
    let actionable = item.mark == PrescanMark::New
        || (copy_mode && item.mark == PrescanMark::Exists);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(cell, cell), egui::Sense::hover());
    let painter = ui.painter();
    let stroke = if resp.hovered() {
        egui::Stroke::new(1.5, theme::ACCENT)
    } else {
        egui::Stroke::new(1.0, theme::BORDER_2)
    };
    painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);

    // Thumbnail (background-generated; Spinner placeholder until ready).
    let inner = rect.shrink(6.0);
    let fake = PhotoListItem {
        id: -(idx as i64) - 1, // synthetic id: never collides with library ids
        original_filename: item.filename.clone(),
        current_path: item.path.clone(),
        folder_path: String::new(),
        status: Status::Untreated,
        capture_time: item.capture_time.clone(),
        file_size: item.file_size,
        thumb_hash: Some(item.thumb_hash.clone()),
        camera_model: None,
        lens_model: None,
        iso: None,
        aperture: None,
        shutter_speed: None,
        focal_length: None,
        decode_failed: false,
        preview_only: false,
        pair_group_id: None,
        rotation_override: 0,
    };
    let (tex, needs) = app.textures.texture_for(ui.ctx(), &fake);
    if needs {
        // 仅生成 256px 缩略图（97 张 × 1920px 预览的 Lanczos 风暴会让
        // worker 满载、画面抖动）；预览图在真正查看时按需生成。
        app.thumbs.enqueue_thumb_only(fake.id, &item.thumb_hash, &item.path);
        egui::Spinner::new().size(22.0).paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), egui::vec2(26.0, 26.0)),
        );
    } else {
        let ts = tex.size_vec2();
        let scale = (inner.width() / ts.x).min(inner.height() / ts.y).min(1.0);
        let img_rect = egui::Rect::from_center_size(inner.center(), ts * scale);
        let tint = if item.mark == PrescanMark::New {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_rgb(150, 150, 150) // 已存在灰显
        };
        painter.image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
    }

    // Checkbox overlay (top-left). 已存在 in add mode is disabled with a
    // tooltip; in copy mode it means 强制导入.
    let cb_rect =
        egui::Rect::from_min_size(rect.min + egui::vec2(5.0, 5.0), egui::vec2(17.0, 17.0));
    let cb = ui.interact(cb_rect, egui::Id::new(("import_scan_cb", idx)), egui::Sense::click());
    painter.rect_filled(
        cb_rect,
        2.0,
        if selected && actionable {
            theme::ACCENT
        } else {
            egui::Color32::from_rgb(0x1e, 0x1e, 0x1e)
        },
    );
    painter.rect_stroke(
        cb_rect,
        2.0,
        egui::Stroke::new(1.0, if actionable { theme::ACCENT } else { theme::BORDER }),
        egui::StrokeKind::Inside,
    );
    if selected && actionable {
        let dark = egui::Stroke::new(2.0, egui::Color32::from_rgb(0x12, 0x12, 0x12));
        painter.line_segment(
            [
                cb_rect.left_top() + egui::vec2(3.0, 8.0),
                cb_rect.center() + egui::vec2(-1.0, 3.0),
            ],
            dark,
        );
        painter.line_segment(
            [
                cb_rect.center() + egui::vec2(-1.0, 3.0),
                cb_rect.right_bottom() + egui::vec2(-3.0, -4.0),
            ],
            dark,
        );
    }
    if cb.clicked() && actionable {
        if selected {
            app.import_scan_selected.remove(&item.path);
        } else {
            app.import_scan_selected.insert(item.path.clone());
        }
    }
    if !actionable {
        cb.on_hover_text(match item.mark {
            PrescanMark::PathRepair => t(
                "已在图库（源路径变化），导入时将自动修复路径。",
                "Already in the library (path changed) — importing repairs the path.",
            ),
            _ => t(
                "已存在于图库，添加模式不支持强制导入。",
                "Already in the library — add mode cannot force-import.",
            ),
        });
    }

    // 底部状态标签。
    if item.mark != PrescanMark::New {
        let (text, color) = if selected && copy_mode {
            (t("强制导入", "Force import"), theme::ACCENT)
        } else if item.mark == PrescanMark::PathRepair {
            (t("路径将修复", "Path repair"), theme::TEXT_WEAK)
        } else {
            (t("已存在，跳过", "Exists, skipped"), theme::TEXT_WEAK)
        };
        let strip = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - 16.0),
            egui::pos2(rect.right(), rect.bottom()),
        );
        painter.rect_filled(strip, 0.0, egui::Color32::from_black_alpha(150));
        painter.text(
            egui::pos2(strip.left() + 4.0, strip.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(10.0),
            color,
        );
    }
}

fn delete_box(app: &mut KakaApp, ctx: &egui::Context) {
    // All pending-delete photos of the current workspace (status = 1), in
    // capture-time order so Shift+click range selection is stable (PRD 8.1).
    let folder = app.state.ws.folder_path.clone();
    let items = db::photos::list_items_in_folder(&app.state.db, &folder, SortOrder::CaptureTimeAsc)
        .unwrap_or_default();
    let deleted: Vec<_> = items.into_iter().filter(|p| p.status == Status::Delete).collect();

    // Ctrl+A / Ctrl+Shift+A act on the delete-box grid while it is open
    // (PRD 8.1). Global shortcuts are already suppressed by the modal guard.
    if !deleted.is_empty() {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::A)) {
            app.delete_sel = deleted.iter().map(|p| p.id).collect();
        }
        if ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::A)
        }) {
            app.delete_sel.clear();
            app.delete_anchor = None;
        }
    }

    dim_backdrop(ctx);
    let mut recycle = false;
    let mut close = false;
    egui::Window::new(t("待删照片", "Photos to delete"))
        .collapsible(false)
        .resizable(true)
        .default_size([960.0, 640.0])
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            let n = deleted.len();
            let groups = deleted
                .iter()
                .filter_map(|p| p.pair_group_id)
                .collect::<HashSet<i64>>()
                .len();
            let title = match i18n::lang() {
                i18n::Lang::Zh => format!("待删照片（{n}张）"),
                i18n::Lang::En => format!("Photos to delete ({n})"),
            };
            ui.label(RichText::new(title).heading().color(theme::TEXT));
            if groups > 0 {
                ui.label(
                    RichText::new(format!(
                        "{}",
                        match i18n::lang() {
                            i18n::Lang::Zh => format!("包含 {groups} 组 RAW+JPG"),
                            i18n::Lang::En => format!("includes {groups} RAW+JPG group(s)"),
                        }
                    ))
                    .size(12.0)
                    .color(theme::TEXT_WEAK),
                );
            }
            ui.label(RichText::new(
                t("单击选中 · Ctrl+单击切换 · Shift+单击范围选 · 双击在预览区查看 · 最终删除会移入回收站（可恢复）并清除数据库记录",
                  "Click to select · Ctrl+click toggle · Shift+click range · double-click to preview · final delete moves files to the recycle bin (recoverable) and removes DB records"))
                .size(12.0).color(theme::TEXT_WEAK));
            ui.separator();

            if deleted.is_empty() {
                // Flow-based empty hint. A full-rect `centered_and_justified`
                // here consumes the whole remaining space and pushes the
                // separator/summary/action bar out of the window — after
                // restoring the last photo the 关闭 button became unreachable.
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.label(RichText::new(t("暂无待删照片", "Nothing marked for deletion")).color(theme::TEXT_WEAK));
                    ui.add_space(80.0);
                });
            } else {
                // Thumbnail grid (PRD 8.1 / UI 5.2): cells scale to the window
                // width, 3–8 per row. The scroll area reserves room for the
                // bottom bar (separator + summary + buttons) so it can never
                // be pushed out of the window.
                let reserved = 110.0f32;
                let mods = ui.input(|i| i.modifiers);
                let ids: Vec<i64> = deleted.iter().map(|p| p.id).collect();
                let cell_w = 150.0f32;
                let cols = ((ui.available_width() / (cell_w + 10.0)).floor() as usize).clamp(3, 8);
                egui::ScrollArea::vertical()
                    .max_height((ui.available_height() - reserved).max(120.0))
                    .show(ui, |ui| {
                    egui::Grid::new("delete_grid")
                        .spacing([10.0, 10.0])
                        .num_columns(cols)
                        .show(ui, |ui| {
                            for (idx, p) in deleted.iter().enumerate() {
                                let selected = app.delete_sel.contains(&p.id);
                                let resp = delete_cell(ui, app, p, selected);
                                if resp.clicked() {
                                    delete_select_click(app, &ids, idx, p.id, mods.ctrl, mods.shift);
                                }
                                if resp.double_clicked() {
                                    // Preview stays live behind the modal (UI spec 5.2).
                                    if let Some(pos) =
                                        app.state.ws.items.iter().position(|w| w.id == p.id)
                                    {
                                        app.state.ws.current_index = pos;
                                    }
                                }
                                if idx % cols == cols - 1 {
                                    ui.end_row();
                                }
                            }
                        });
                });
            }

            ui.separator();
            let n = deleted.len();
            let sel = app.delete_sel.len();
            let freed = deleted.iter().map(|p| p.file_size).sum::<i64>();
            ui.horizontal(|ui| {
                let summary = match i18n::lang() {
                    i18n::Lang::Zh => format!("选中 {sel} 张 · 共 {n} 张 · 预计释放空间 {}",
                        crate::app::copy::human_bytes(freed)),
                    i18n::Lang::En => format!("selected {sel} · total {n} · estimated space freed {}",
                        crate::app::copy::human_bytes(freed)),
                };
                ui.label(RichText::new(summary).size(13.0).color(theme::TEXT_SECONDARY));
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if n > 0 {
                    // 恢复选中 (PRD 8.2): only enabled with a selection.
                    let restore_btn = egui::Button::new(
                        RichText::new(format!("{} ({sel})", t("恢复选中", "Restore selected"))).color(theme::KEEP),
                    );
                    if ui.add_enabled(sel > 0, restore_btn).clicked() {
                        let ids: Vec<i64> = deleted
                            .iter()
                            .filter(|p| app.delete_sel.contains(&p.id))
                            .map(|p| p.id)
                            .collect();
                        let _ = db::photos::set_status_batch(&app.state.db, &ids, Status::Untreated);
                        let _ = app.state.reload_current();
                        app.delete_sel.clear();
                        app.delete_anchor = None;
                        let msg = match i18n::lang() {
                            i18n::Lang::Zh => format!("已恢复 {sel} 张为未处理"),
                            i18n::Lang::En => format!("Restored {sel} photo(s) to unprocessed"),
                        };
                        app.toast(ToastKind::Success, msg);
                        app.needs_save = true;
                    }
                    // 全部恢复: reset every marked photo (PRD 8.2).
                    if ui
                        .add(egui::Button::new(
                            RichText::new(format!("{} ({n})", t("全部恢复", "Restore all"))).color(theme::KEEP),
                        ))
                        .clicked()
                    {
                        let ids: Vec<i64> = deleted.iter().map(|p| p.id).collect();
                        let _ = db::photos::set_status_batch(&app.state.db, &ids, Status::Untreated);
                        let _ = app.state.reload_current();
                        app.delete_sel.clear();
                        app.delete_anchor = None;
                        app.state.show_delete_box = false;
                    }
                    // Final delete (PRD 8.3): recycle the files, clear the records.
                    if ui
                        .add(egui::Button::new(
                            RichText::new(format!("{} ({n})", t("全部移入回收站", "Move all to recycle bin")))
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(theme::DELETE)
                        .stroke(egui::Stroke::new(1.0, theme::DELETE)))
                        .clicked()
                    {
                        recycle = true;
                    }
                }
                if ui.button(t("关闭", "Close")).clicked() {
                    close = true;
                }
            });
        });

    if close {
        app.delete_sel.clear();
        app.delete_anchor = None;
        app.state.show_delete_box = false;
    }

    if recycle {
        let paths: Vec<std::path::PathBuf> =
            deleted.iter().map(|p| std::path::PathBuf::from(&p.current_path)).collect();
        let ids: Vec<i64> = deleted.iter().map(|p| p.id).collect();
        let n = paths.len();
        app.confirm = Some(ConfirmDialog {
            title: t("移入回收站", "Move to recycle bin").into(),
            text: match i18n::lang() {
                i18n::Lang::Zh => format!("确认将 {n} 张照片及其文件移入回收站？此操作可从回收站恢复，并会清除数据库记录。"),
                i18n::Lang::En => format!("Move {n} photos and their files to the recycle bin? They can be restored from there; database records will be removed."),
            },
            confirm_label: t("移入回收站", "Recycle").into(),
            danger: true,
            on_confirm: Box::new(move |app| {
                // PRD 8.3: files that no longer exist are only removed from the
                // DB and reported separately.
                let (existing, missing): (Vec<std::path::PathBuf>, Vec<std::path::PathBuf>) =
                    paths.iter().cloned().partition(|p| p.exists());
                let ok = existing.is_empty()
                    || crate::io::recycle::move_to_recycle_bin(&existing).is_ok();
                for id in &ids {
                    let _ = db::photos::delete_photo(&app.state.db, *id);
                }
                let _ = app.state.reload_current();
                app.delete_sel.clear();
                app.delete_anchor = None;
                app.state.show_delete_box = false;
                if ok {
                    let msg = match i18n::lang() {
                        i18n::Lang::Zh => format!(
                            "已将 {} 张照片移入回收站，清理 {} 条记录；{} 张文件已不存在，仅清理数据库记录",
                            existing.len(),
                            ids.len(),
                            missing.len()
                        ),
                        i18n::Lang::En => format!(
                            "Moved {} photo(s) to the recycle bin, removed {} record(s); {} file(s) were already gone — DB records cleaned only",
                            existing.len(),
                            ids.len(),
                            missing.len()
                        ),
                    };
                    app.toast(ToastKind::Success, msg);
                } else {
                    app.toast(ToastKind::Error, t("部分照片移入回收站失败，请检查回收站状态", "Some files failed to move to the recycle bin"));
                }
                app.needs_save = true;
            }),
        });
    }
}

/// Delete-box selection click (PRD 8.1): plain click = select one, Ctrl+click
/// = toggle, Shift+click = range from the anchor over the capture-time-ordered
/// grid (`ids`). The selection is scoped to this dialog (`app.delete_sel`),
/// independent of the workspace selection.
fn delete_select_click(app: &mut KakaApp, ids: &[i64], idx: usize, id: i64, ctrl: bool, shift: bool) {
    if shift {
        let anchor = app.delete_anchor.unwrap_or(idx);
        let (lo, hi) = (anchor.min(idx), anchor.max(idx));
        for i in lo..=hi {
            if let Some(&sel_id) = ids.get(i) {
                app.delete_sel.insert(sel_id);
            }
        }
        app.delete_anchor = Some(anchor);
    } else if ctrl {
        if app.delete_sel.contains(&id) {
            app.delete_sel.remove(&id);
        } else {
            app.delete_sel.insert(id);
        }
        app.delete_anchor = Some(idx);
    } else {
        app.delete_sel.clear();
        app.delete_sel.insert(id);
        app.delete_anchor = Some(idx);
    }
}

/// One grid cell: thumbnail + rotation, checkbox, 待删/R+J badges, missing-file
/// overlay and truncated filename (PRD 8.1 / UI 5.2). Returns the cell response
/// for click / double-click handling.
fn delete_cell(
    ui: &mut egui::Ui,
    app: &mut KakaApp,
    p: &PhotoListItem,
    selected: bool,
) -> egui::Response {
    let size = egui::vec2(150.0, 152.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter();

    // Image canvas with a filename strip below.
    let canvas = egui::Rect::from_min_size(
        rect.min + egui::vec2(6.0, 6.0),
        egui::vec2(size.x - 12.0, 118.0),
    );
    painter.rect_filled(canvas, 0.0, theme::PREVIEW_BG);

    let (tex, needs) = app.textures.texture_for(ui.ctx(), p);
    if needs {
        let hash = p.thumb_hash.clone().unwrap_or_default();
        app.thumbs.enqueue(p.id, &hash, &p.current_path);
    }
    let ts = tex.size_vec2();
    if ts.x > 0.0 && ts.y > 0.0 {
        // Respect the manual rotation here too (PRD 4.7): swap the fit box for
        // 90/270 turns, never upscale beyond the cached thumb.
        let turns = p.rotation_override.rem_euclid(4);
        let swapped = turns % 2 == 1;
        let (bw, bh) = if swapped {
            (canvas.height(), canvas.width())
        } else {
            (canvas.width(), canvas.height())
        };
        let scale = (bw / ts.x).min(bh / ts.y).min(1.0);
        let dsize = egui::vec2(ts.x * scale, ts.y * scale);
        if turns == 0 {
            painter.image(
                tex.id(),
                egui::Rect::from_center_size(canvas.center(), dsize),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            super::view::draw_image_rotated(painter, tex.id(), canvas.center(), dsize, turns, egui::Color32::WHITE);
        }
    }

    // 文件丢失 (PRD 8.1 c): dim the canvas + a broken-file hint.
    let missing = !std::path::Path::new(&p.current_path).exists();
    if missing {
        painter.rect_filled(
            canvas,
            0.0,
            egui::Color32::from_rgba_unmultiplied(10, 10, 10, 200),
        );
        painter.text(
            canvas.center(),
            Align2::CENTER_CENTER,
            t("⚠ 文件丢失", "⚠ File missing"),
            egui::FontId::proportional(12.0),
            theme::TEXT_SECONDARY,
        );
    }

    // Checkbox, top-left 14x14 (PRD 8.1): accent-filled with a tick when on.
    let cb = egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), egui::vec2(14.0, 14.0));
    if selected {
        painter.rect_filled(cb, 0.0, theme::ACCENT);
        painter.text(
            cb.center(),
            Align2::CENTER_CENTER,
            "✓",
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(0x12, 0x12, 0x12),
        );
    } else {
        painter.rect_stroke(cb, 0.0, egui::Stroke::new(1.0, theme::BORDER_2), egui::StrokeKind::Inside);
    }

    // 待删 badge, top-right (small variant per UI spec 3.5.3).
    let badge = egui::Rect::from_min_size(
        egui::pos2(rect.max.x - 36.0, rect.min.y),
        egui::vec2(36.0, 18.0),
    );
    painter.rect_filled(badge, 0.0, theme::DELETE);
    painter.text(
        badge.center(),
        Align2::CENTER_CENTER,
        t("待删", "Del"),
        egui::FontId::proportional(10.0),
        egui::Color32::WHITE,
    );
    if p.pair_group_id.is_some() {
        painter.text(
            egui::pos2(rect.max.x - 4.0, canvas.max.y - 2.0),
            Align2::RIGHT_BOTTOM,
            "R+J",
            egui::FontId::proportional(10.0),
            theme::TEXT,
        );
    }

    // Filename strip.
    painter.text(
        egui::pos2(rect.center().x, canvas.max.y + 9.0),
        Align2::CENTER_CENTER,
        truncate_name(&p.original_filename, 22),
        egui::FontId::proportional(11.0),
        theme::TEXT_WEAK,
    );

    // Border: hover = accent 1px (UI spec 5.2 悬停), selected = accent 2px.
    let stroke = if selected {
        egui::Stroke::new(2.0, theme::ACCENT)
    } else if resp.hovered() {
        egui::Stroke::new(1.0, theme::ACCENT)
    } else {
        egui::Stroke::new(1.0, theme::BORDER)
    };
    painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
    resp
}

fn truncate_name(name: &str, max: usize) -> String {
    let count = name.chars().count();
    if count <= max {
        return name.to_string();
    }
    let head: String = name.chars().take(max - 1).collect();
    format!("{head}…")
}

fn confirm_dialog(app: &mut KakaApp, ctx: &egui::Context) {
    let Some(c) = app.confirm.as_ref() else { return; };
    dim_backdrop(ctx);
    let text = c.text.clone();
    let title = c.title.clone();
    let label = c.confirm_label.clone();
    let danger = c.danger;
    let mut confirm_clicked = false;
    let mut cancel_clicked = false;
    egui::Window::new(t("确认", "Confirm"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([460.0, 220.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            ui.label(RichText::new(title).heading().color(theme::TEXT));
            ui.separator();
            ui.label(RichText::new(text).size(14.0).color(theme::TEXT_SECONDARY));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let btn = if danger {
                    egui::Button::new(RichText::new(label).strong().color(egui::Color32::WHITE))
                        .fill(theme::DELETE)
                        .stroke(egui::Stroke::new(1.0, theme::DELETE))
                } else {
                    egui::Button::new(RichText::new(label).strong().color(egui::Color32::from_rgb(0x12, 0x12, 0x12)))
                        .fill(theme::ACCENT)
                        .stroke(egui::Stroke::new(1.0, theme::ACCENT))
                };
                if ui.add(btn).clicked() {
                    confirm_clicked = true;
                }
                if ui.button(t("取消", "Cancel")).clicked() {
                    cancel_clicked = true;
                }
            });
        });
    // Consume only on an explicit button press, so the dialog stays up until then.
    if confirm_clicked {
        if let Some(c) = app.confirm.take() {
            (c.on_confirm)(app);
        }
    } else if cancel_clicked {
        app.confirm = None;
    }
}

fn render_toasts(app: &mut KakaApp, ctx: &egui::Context) {
    if app.toasts.is_empty() {
        return;
    }
    // Newest three; right-click a toast to dismiss it immediately.
    let shown: Vec<usize> = (0..app.toasts.len()).rev().take(3).collect();
    let mut dismiss: Option<usize> = None;
    egui::Area::new("toasts".into())
        .anchor(Align2::RIGHT_TOP, [-16.0, 16.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            for &i in &shown {
                let toast = &app.toasts[i];
                let bar = match toast.kind {
                    ToastKind::Info | ToastKind::Warning => theme::ACCENT,
                    ToastKind::Success => theme::KEEP,
                    ToastKind::Error => theme::DELETE,
                };
                let text = RichText::new(&toast.text).size(14.0).color(theme::TEXT);
                let frame = egui::Frame::default()
                    .fill(egui::Color32::from_rgba_unmultiplied(30, 30, 30, 0xf2))
                    .stroke(egui::Stroke::new(1.0, theme::BORDER_2))
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(3.0, 20.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 0.0, bar);
                            ui.label(text);
                        });
                    });
                let resp = frame
                    .response
                    .interact(egui::Sense::click())
                    .on_hover_text(t("右键关闭", "Right-click to dismiss"));
                if resp.secondary_clicked() {
                    dismiss = Some(i);
                }
            }
        });
    if let Some(i) = dismiss {
        app.toasts.remove(i);
    }
}

fn dialog_frame() -> egui::Frame {
    egui::Frame::default()
        .fill(egui::Color32::from_rgb(0x1f, 0x1f, 0x1f))
        .stroke(egui::Stroke::new(1.0, theme::BORDER_2))
        .inner_margin(16.0)
}

fn primary_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).strong().color(egui::Color32::from_rgb(0x12, 0x12, 0x12)))
        .fill(theme::ACCENT)
        .stroke(egui::Stroke::new(1.0, theme::ACCENT))
}
