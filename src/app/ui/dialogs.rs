//! Modal dialogs: import, crash recovery, settings, delete box, confirm, toasts.

use super::app::{KakaApp, ToastKind};
use super::theme;
use crate::model::{SortOrder, Status};
use crate::db;
use eframe::egui::{self, Align2, RichText};
use std::sync::atomic::Ordering;

pub fn render_dialogs(app: &mut KakaApp, ctx: &egui::Context) {
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
    if app.state.show_delete_box {
        delete_box(app, ctx);
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
    egui::Window::new("断点续传")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([520.0, 260.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            ui.label(RichText::new("检测到上次有未完成的导入任务").size(20.0).strong().color(theme::TEXT));
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "共 {total} 张，已完成 {done} 张。\n是否从断点继续导入到：{target}"
                ))
                .size(14.0)
                .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add(primary_button("继续导入")).clicked() {
                    let opts = session.copy_options();
                    // Reopen the import dialog showing copy mode so the user sees progress.
                    app.import_mode = crate::app::state::ImportMode::Copy;
                    app.import_path = source.clone();
                    app.import_target = target.clone();
                    app.import_org = opts.org_mode;
                    app.state.show_import = true;
                    app.start_copy_import(&source, opts, Some(session.clone()));
                    app.show_resume = false;
                    app.pending_resume = None;
                }
                if ui
                    .button(RichText::new("放弃本次任务").color(theme::DELETE))
                    .clicked()
                {
                    let mut s = session;
                    let _ = crate::app::session::abandon(&mut s);
                    app.show_resume = false;
                    app.pending_resume = None;
                    app.toast(ToastKind::Info, "已放弃本次未完成的导入任务");
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

fn import_dialog(app: &mut KakaApp, ctx: &egui::Context) {
    dim_backdrop(ctx);
    egui::Window::new("导入照片")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([680.0, 460.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            ui.label(RichText::new("导入照片").heading().color(theme::TEXT));
            // Mode tabs.
            ui.horizontal(|ui| {
                let add_sel = app.import_mode == crate::app::state::ImportMode::Add;
                if ui.selectable_label(add_sel, "添加模式（从硬盘）").clicked() {
                    app.import_mode = crate::app::state::ImportMode::Add;
                }
                if ui.selectable_label(!add_sel, "复制模式（从存储卡）").clicked() {
                    app.import_mode = crate::app::state::ImportMode::Copy;
                }
            });
            ui.separator();
            match app.import_mode {
                crate::app::state::ImportMode::Add => {
                    ui.label(
                        RichText::new("添加模式：将已有文件夹的照片添加到图库（仅索引，不拷贝文件）。")
                            .size(14.0)
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new("源路径").size(13.0).color(theme::TEXT_SECONDARY));
                    ui.horizontal(|ui| {
                        let mut path = app.import_path.clone();
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut path)
                                .desired_width(520.0)
                                .hint_text("选择或粘贴要添加的文件夹路径"),
                        );
                        if resp.changed() {
                            app.import_path = path;
                        }
                        if ui.button("浏览…").clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                app.import_path = p.to_string_lossy().into_owned();
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut app.import_recursive, "递归扫描子文件夹");
                        ui.checkbox(&mut app.import_dedup, "去重扫描");
                    });
                }
                crate::app::state::ImportMode::Copy => {
                    ui.label(
                        RichText::new("复制模式：从存储卡/文件夹导入，物理拷贝照片到目标目录，保留原文件名。")
                            .size(14.0)
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new("源路径").size(13.0).color(theme::TEXT_SECONDARY));
                    ui.horizontal(|ui| {
                        let mut path = app.import_path.clone();
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut path)
                                .desired_width(480.0)
                                .hint_text("选择要导入的文件夹/存储卡"),
                        );
                        if resp.changed() {
                            app.import_path = path;
                        }
                        if ui.button("浏览…").clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                app.import_path = p.to_string_lossy().into_owned();
                            }
                        }
                    });
                    ui.label(RichText::new("目标目录").size(13.0).color(theme::TEXT_SECONDARY));
                    ui.horizontal(|ui| {
                        let mut target = app.import_target.clone();
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut target)
                                .desired_width(430.0)
                                .hint_text("选择目标目录"),
                        );
                        if resp.changed() {
                            app.import_target = target;
                        }
                        if ui.button("浏览…").clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                app.import_target = p.to_string_lossy().into_owned();
                            }
                        }
                    });
                    ui.label(RichText::new("子目录组织方式").size(13.0).color(theme::TEXT_SECONDARY));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut app.import_org, crate::app::copy::OrgMode::Structure, "保持原结构");
                        ui.radio_value(&mut app.import_org, crate::app::copy::OrgMode::Date, "按拍摄日期");
                        ui.radio_value(&mut app.import_org, crate::app::copy::OrgMode::Flat, "全部平铺");
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut app.import_recursive, "递归扫描子文件夹");
                        ui.checkbox(&mut app.import_dedup, "去重扫描");
                    });
                    // 清空存储卡 (PRD 6.3): only enabled when the source is a
                    // removable device; otherwise grayed out and forced off.
                    let removable =
                        crate::app::card::is_removable_source(std::path::Path::new(&app.import_path));
                    if !removable {
                        app.import_clear_card = false;
                    }
                    let checkbox = egui::Checkbox::new(&mut app.import_clear_card, "导入后清空存储卡");
                    let resp = ui.add_enabled(removable, checkbox).on_hover_text(
                        "导入完成且全部成功者会移入回收站（非永久删除）。仅当源路径为可移动存储设备时可用。",
                    );
                    let _ = resp;
                }
            }
            ui.separator();

            if app.state.import_running {
                let p = app.state.import_progress.clone();
                let frac = if p.total > 0 { p.done as f32 / p.total as f32 } else { 0.0 };
                let phase_text = match p.phase.as_str() {
                    "检查" => "正在检查与分析照片…",
                    "拷贝" => "正在拷贝照片…",
                    "扫描" => "正在扫描照片…",
                    "准备" => "正在准备…",
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
                if ui.button("取消导入").clicked() {
                    app.import_cancel.store(true, Ordering::SeqCst);
                }
            } else {
                let btn = egui::Button::new(RichText::new("导入").size(15.0).strong().color(egui::Color32::from_rgb(0x12, 0x12, 0x12)))
                    .fill(theme::ACCENT)
                    .stroke(egui::Stroke::new(1.0, theme::ACCENT));
                if ui.add(btn).clicked() {
                    let path = app.import_path.clone();
                    if path.trim().is_empty() {
                        app.toast(ToastKind::Warning, "请选择源路径");
                    } else {
                        match app.import_mode {
                            crate::app::state::ImportMode::Add => {
                                app.start_add_import(path.trim());
                            }
                            crate::app::state::ImportMode::Copy => {
                                if app.import_target.trim().is_empty() {
                                    app.toast(ToastKind::Warning, "请选择目标目录");
                                } else {
                                    let opts = crate::app::copy::CopyOptions {
                                        target_dir: app.import_target.clone(),
                                        org_mode: app.import_org,
                                        recursive: app.import_recursive,
                                        dedup: app.import_dedup,
                                        clear_card: app.import_clear_card,
                                    };
                                    app.start_copy_import(path.trim(), opts, None);
                                }
                            }
                        }
                    }
                }
            }

            if let Some(res) = &app.state.import_result {
                ui.separator();
                match res {
                    Ok(outcome) => {
                        match outcome {
                            crate::app::state::ImportResult::Add(o) => {
                                ui.label(RichText::new(format!(
                                    "已将 {} 张照片添加到图库（跳过已存在 {} 张，失败 {} 张，路径修复 {} 条）",
                                    o.added, o.skipped_existing, o.failed, o.path_repaired
                                )).size(14.0).color(theme::KEEP));
                                if !o.failures.is_empty() {
                                    ui.label(RichText::new("失败列表（前3条）：").size(12.0).color(theme::DELETE));
                                    for f in o.failures.iter().take(3) {
                                        ui.label(RichText::new(f).size(12.0).color(theme::TEXT_WEAK));
                                    }
                                }
                            }
                            crate::app::state::ImportResult::Copy(o) => {
                                ui.label(RichText::new(format!(
                                    "成功导入 {} 张（已存在跳过 {} 张，失败 {} 张）",
                                    o.copied, o.skipped_existing, o.failed
                                )).size(14.0).color(theme::KEEP));
                                if !o.failures.is_empty() {
                                    ui.label(RichText::new("失败列表（前3条）：").size(12.0).color(theme::DELETE));
                                    for f in o.failures.iter().take(3) {
                                        ui.label(RichText::new(f).size(12.0).color(theme::TEXT_WEAK));
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        ui.label(RichText::new(format!("导入失败：{e}")).size(14.0).color(theme::DELETE));
                    }
                }
                if ui.button("关闭").clicked() {
                    app.state.show_import = false;
                }
            } else {
                if ui.button("取消").clicked() {
                    app.state.show_import = false;
                }
            }
        });
    ctx.request_repaint();
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
    egui::Window::new("恢复")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([560.0, 340.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            ui.label(RichText::new("检测到上次非正常关闭").size(20.0).strong().color(theme::TEXT));
            ui.add_space(8.0);
            let folder = state.current_folder_path.clone().unwrap_or_default();
            let (total, processed) = counts;
            ui.label(
                RichText::new(format!(
                    "上次工作区：{folder}\n已处理 {processed}/{total} 张。是否恢复上次的标记和浏览位置？"
                ))
                .size(14.0)
                .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(6.0);
            ui.label(RichText::new("崩溃不会删除照片文件，仅可能丢失未保存的筛选标记。").size(12.0).color(theme::TEXT_WEAK));
            ui.add_space(10.0);

            let mut close = false;
            ui.horizontal(|ui| {
                if ui.add(primary_button("恢复并继续")).clicked() {
                    // Restore saved position.
                    if let Some(folder) = state.current_folder_path.clone() {
                        let sort = SortOrder::from_code(&state.current_sort);
                        let _ = app.state.open_workspace(&folder, sort);
                        app.state.ws.current_index = state.current_index.max(0) as usize;
                    }
                    app.settle_crash_recovery();
                    close = true;
                }
                if ui.button("保留标记，从头浏览").clicked() {
                    if let Some(folder) = state.current_folder_path.clone() {
                        let sort = SortOrder::from_code(&state.current_sort);
                        let _ = app.state.open_workspace(&folder, sort);
                        app.state.ws.current_index = 0;
                    }
                    app.settle_crash_recovery();
                    close = true;
                }
                if ui.button(RichText::new("放弃所有未保存的标记，重置为未处理").color(theme::DELETE)).clicked() {
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
    dim_backdrop(ctx);
    egui::Window::new("设置")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([720.0, 520.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            ui.label(RichText::new("设置").heading().color(theme::TEXT));
            ui.separator();
            ui.label(RichText::new("常规").size(13.0).color(theme::ACCENT).strong());
            ui.horizontal(|ui| {
                ui.checkbox(&mut app.settings_draft.auto_open_last_workspace, "自动打开上次工作区");
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut app.settings_draft.dim_reviewed_thumbnails, "缩略图淡化已阅跳过");
            });
            ui.checkbox(&mut app.settings_draft.batch_confirm, "批量操作二次确认");
            ui.add_space(8.0);
            ui.label(RichText::new("关于").size(13.0).color(theme::ACCENT).strong());
            ui.label(RichText::new("咔咔 v0.1.0（M1 MVP）").size(14.0).color(theme::TEXT));
            ui.label(RichText::new("开源 · 无自动更新 · Windows x86_64").size(12.0).color(theme::TEXT_WEAK));
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("保存").clicked() {
                    save = true;
                    app.state.show_settings = false;
                }
                if ui.button("取消").clicked() {
                    // Discard draft, keep existing config.
                    app.state.show_settings = false;
                }
            });
        });
    if save {
        // Apply draft to live config + persist.
        app.state.config = app.settings_draft.clone();
        if let Err(e) = crate::config::save(&app.state.config) {
            app.toast(ToastKind::Error, format!("设置保存失败：{e}"));
        } else {
            app.toast(ToastKind::Success, "设置已保存");
        }
    }
}

fn delete_box(app: &mut KakaApp, ctx: &egui::Context) {
    // List all pending-delete photos for the current workspace (status = 1).
    let folder = app.state.ws.folder_path.clone();
    let items = db::photos::list_items_in_folder(&app.state.db, &folder, SortOrder::CaptureTimeAsc)
        .unwrap_or_default();
    let deleted: Vec<_> = items.into_iter().filter(|p| p.status == Status::Delete).collect();

    dim_backdrop(ctx);
    egui::Window::new("待删照片")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([760.0, 520.0])
        .frame(dialog_frame())
        .show(ctx, |ui| {
            ui.label(RichText::new(format!("待删照片（{}张）", deleted.len())).heading().color(theme::TEXT));
            ui.label(RichText::new("M1：仅支持恢复。最终移入回收站将在后续里程碑提供。").size(12.0).color(theme::TEXT_WEAK));
            ui.separator();
            if deleted.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("暂无待删照片").color(theme::TEXT_WEAK));
                });
            } else {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for p in &deleted {
                            ui.label(RichText::new(format!("🗑 {}", p.original_filename)).size(13.0).color(theme::TEXT_SECONDARY));
                        }
                    });
                });
            }
            ui.separator();
            let n = deleted.len();
            if ui
                .add(egui::Button::new(RichText::new(format!("全部恢复（{n}）")).color(theme::KEEP)))
                .clicked()
            {
                if let Ok(ids) = db::photos::list_ids_in_folder(&app.state.db, &folder, SortOrder::CaptureTimeAsc) {
                    let _ = db::photos::set_status_batch(&app.state.db, &ids, Status::Untreated);
                    let _ = app.state.reload_current();
                }
                app.state.show_delete_box = false;
            }
            if ui.button("关闭").clicked() {
                app.state.show_delete_box = false;
            }
        });
}

fn confirm_dialog(app: &mut KakaApp, ctx: &egui::Context) {
    let Some(c) = app.confirm.take() else { return; };
    dim_backdrop(ctx);
    let text = c.text.clone();
    let title = c.title.clone();
    let label = c.confirm_label.clone();
    let danger = c.danger;
    let run = c.on_confirm;
    egui::Window::new("确认")
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
                    run(app);
                }
                if ui.button("取消").clicked() {
                    // do nothing
                }
            });
        });
}

fn render_toasts(app: &mut KakaApp, ctx: &egui::Context) {
    let toasts = &app.toasts;
    if toasts.is_empty() {
        return;
    }
    egui::Area::new("toasts".into())
        .anchor(Align2::RIGHT_TOP, [-16.0, 16.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            for t in toasts.iter().rev().take(3) {
                let bar = match t.kind {
                    ToastKind::Info | ToastKind::Warning => theme::ACCENT,
                    ToastKind::Success => theme::KEEP,
                    ToastKind::Error => theme::DELETE,
                };
                let text = RichText::new(&t.text).size(14.0).color(theme::TEXT);
                egui::Frame::default()
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
            }
        });
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
