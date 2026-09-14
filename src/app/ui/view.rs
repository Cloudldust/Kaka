//! Main window layout (UI Spec 2, 3): top bar, progress, preview, right panel,
//! thumbnail strip, status bar, empty state.

use super::app::{ConfirmDialog, KakaApp, ToastKind};
use super::theme;
use crate::i18n::t;
use crate::model::{PhotoListItem, SortOrder, Status};
use eframe::egui::{self, Align, Align2, Layout, RichText};

pub fn render(app: &mut KakaApp, ui: &mut egui::Ui) {
    render_top_bottom_panels(app, ui);
}

fn render_top_bottom_panels(app: &mut KakaApp, ui: &mut egui::Ui) {
    // UI 3.1: 搜索框 300ms 防抖——输入停顿后应用（回车则即时触发，见搜索框）。
    let debounce_ready = app
        .search_pending
        .as_ref()
        .map(|(_, at)| at.elapsed().as_millis() >= 300)
        .unwrap_or(false);
    if debounce_ready {
        let text = app.search_pending.take().map(|(t, _)| t).unwrap_or_default();
        apply_search(app, &text);
    }
    let has_ws = app.state.folder_loaded && !app.state.ws.items.is_empty();

    // ---- Top bar ----
    egui::Panel::top("top_bar")
        .default_size(theme::TOP_BAR_HEIGHT)
        .size_range(egui::Rangef::new(theme::TOP_BAR_HEIGHT, theme::TOP_BAR_HEIGHT))
        .frame(frame_pad(theme::TOP_BAR_BG, 16, 0))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("📷").size(20.0));
                ui.label(
                    RichText::new("咔咔")
                        .size(20.0)
                        .strong()
                        .color(theme::TEXT),
                );

                ui.separator();

                let path = app.state.ws.folder_path.clone();
                // UI 3.1-2: 路径下拉（最近 10 文件夹 / 浏览 / 复制完整路径）+
                // Ctrl+L 内联编辑。
                if app.path_edit_active {
                    let mut edit = app.path_edit.clone();
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut edit)
                            .desired_width(360.0)
                            .hint_text(t("输入文件夹路径，回车打开", "Type a folder path, Enter to open")),
                    );
                    if resp.changed() {
                        app.path_edit = edit;
                    }
                    let enter = resp.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    if enter {
                        let f = app.path_edit.clone();
                        app.open_folder(&f);
                        app.path_edit_active = false;
                    } else if esc {
                        app.path_edit_active = false;
                    }
                } else {
                    let label = truncate_path(&path, 44);
                    let fmt: String = if path.is_empty() {
                        t("未打开文件夹", "No folder open").to_string()
                    } else {
                        label
                    };
                    let hover: String = if path.is_empty() {
                        t("点击选择/浏览文件夹 · Ctrl+L 输入路径", "Click to pick / browse · Ctrl+L to type a path").to_string()
                    } else {
                        path.clone()
                    };
                    ui.menu_button(RichText::new(fmt).size(14.0).color(theme::TEXT), |ui| {
                        for f in app.recent_folders().into_iter().take(10) {
                            let fp = f.folder_path.clone();
                            if ui.button(RichText::new(&fp).size(13.0)).clicked() {
                                app.open_folder(&fp);
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui
                            .button(RichText::new(t("浏览文件夹…", "Browse for folder…")).size(13.0))
                            .clicked()
                        {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                let p = p.to_string_lossy().into_owned();
                                app.open_folder(&p);
                            }
                            ui.close();
                        }
                        if ui
                            .button(RichText::new(t("复制完整路径", "Copy full path")).size(13.0))
                            .clicked()
                        {
                            ui.ctx().copy_text(path.clone());
                            ui.close();
                        }
                    })
                    .response
                    .on_hover_text(hover);
                }

                ui.separator();
                sort_dropdown(app, ui);

                ui.add_space(8.0);
                ui.label(RichText::new("🔍").size(14.0).color(theme::TEXT_SECONDARY));
                let mut search = app.state.ws.search.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut search)
                        .desired_width(200.0)
                        .hint_text(t("搜索文件名 / @待删/已阅/未处理/丢失/配对 · &&与 ||或 !非", "Search file name / @delete/reviewed/untreated/missing/paired · && AND || OR ! NOT")),
                );
                // UI 3.1: 输入即 300ms 防抖，回车立即触发。
                if resp.changed() {
                    app.state.ws.search = search.clone();
                    app.search_pending = Some((search.clone(), std::time::Instant::now()));
                }
                let enter = resp.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if enter {
                    app.search_pending = None;
                    apply_search(app, &search);
                }
                // @ 自动补全：输入以 `@` 结尾且聚焦时，在搜索框下方列出可用关键词，点击即填入。
                if resp.has_focus() && search.trim_end().ends_with('@') {
                    let anchor = egui::pos2(resp.rect.left(), resp.rect.bottom() + 2.0);
                    egui::Area::new(egui::Id::new("search_at_suggest"))
                        .fixed_pos(anchor)
                        .order(egui::Order::Foreground)
                        .show(ui.ctx(), |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| {
                                let lang_zh = crate::i18n::lang() == crate::i18n::Lang::Zh;
                                let kws: [(&str, &str); 5] = [
                                    ("待删", "delete"),
                                    ("已阅", "reviewed"),
                                    ("未处理", "untreated"),
                                    ("丢失", "missing"),
                                    ("配对", "paired"),
                                ];
                                for (zh, en) in kws {
                                    let kw = if lang_zh { zh } else { en };
                                    let label = format!("@{kw}");
                                    if ui
                                        .button(RichText::new(&label).size(13.0).color(theme::TEXT))
                                        .clicked()
                                    {
                                        // 用 @关键词 替换末尾的 `@`。
                                        let head = search.trim_end()[..search.trim_end().len() - 1].to_string();
                                        let filled = if head.trim().is_empty() {
                                            label.clone()
                                        } else {
                                            format!("{head} {label}")
                                        };
                                        app.state.ws.search = filled.clone();
                                        app.search_pending = None;
                                        apply_search(app, &filled);
                                    }
                                }
                            });
                        });
                }
                if !app.state.ws.search.is_empty() && ui.button("✕").clicked() {
                    app.search_pending = None;
                    app.state.ws.search.clear();
                    apply_search(app, "");
                }

                // Advanced filter (PRD 7.8) button.
                let filter_active = app.state.ws.filter.is_active();
                let filter_btn_style = if filter_active {
                    egui::Button::new(
                        RichText::new(t("过滤", "Filter")).strong().color(egui::Color32::from_rgb(0x12, 0x12, 0x12)),
                    )
                    .fill(theme::ACCENT)
                    .stroke(egui::Stroke::new(1.0, theme::ACCENT))
                } else {
                    egui::Button::new(RichText::new(t("过滤", "Filter")).color(theme::TEXT_SECONDARY))
                };
                if ui.add(filter_btn_style).clicked() {
                    app.filter_draft = app.state.ws.filter.clone();
                    app.state.show_filter = true;
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let del = app.state.ws.counts.deleted;
                    let del_text = format!("{} ({del})", t("待删", "Delete"));
                    let btn = if del > 0 {
                        egui::Button::new(
                            RichText::new(del_text).strong().color(egui::Color32::WHITE),
                        )
                        .fill(theme::DELETE)
                        .stroke(egui::Stroke::new(1.0, theme::DELETE))
                    } else {
                        egui::Button::new(RichText::new(del_text).color(theme::TEXT_WEAK))
                    };
                    if ui.add(btn).clicked() {
                        app.state.show_delete_box = true;
                    }

                    let import_btn = egui::Button::new(
                        RichText::new(t("导入", "Import"))
                            .size(15.0)
                            .strong()
                            .color(egui::Color32::from_rgb(0x12, 0x12, 0x12)),
                    )
                    .fill(theme::ACCENT)
                    .stroke(egui::Stroke::new(1.0, theme::ACCENT));
                    if ui.add(import_btn).clicked() {
                        app.state.show_import = true;
                    }

                    // 导出 (PRD 12): only with an open workspace.
                    let export_btn = egui::Button::new(RichText::new(t("导出", "Export")).size(15.0).color(theme::TEXT_SECONDARY));
                    let resp = ui.add_enabled(has_ws, export_btn);
                    if resp.clicked() {
                        if app.export_target.trim().is_empty() {
                            if !app.state.config.default_target_dir.trim().is_empty() {
                                app.export_target = app.state.config.default_target_dir.clone();
                            } else {
                                app.export_target = app.state.ws.folder_path.clone();
                            }
                        }
                        app.state.show_export = true;
                        app.lr_path = crate::app::export::lr_install_path(&app.state.config.lr_install_path);
                    }
                });
            });
        });

    // ---- Progress bar (4px) ----
    egui::Panel::top("progress")
        .default_size(theme::PROGRESS_HEIGHT)
        .size_range(egui::Rangef::new(theme::PROGRESS_HEIGHT, theme::PROGRESS_HEIGHT))
        .frame(frame_fill(theme::BG))
        .show(ui, |ui| {
            let processed = app.state.ws.counts.deleted + app.state.ws.counts.reviewed;
            let total = app.state.ws.counts.total.max(1);
            let frac = (processed as f32 / total as f32).clamp(0.0, 1.0);
            let (fill, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), theme::PROGRESS_HEIGHT),
                egui::Sense::hover(),
            );
            ui.painter().rect_filled(fill, 0.0, theme::BG);
            let green = processed >= total && total > 0;
            // PRD 7.3: 筛选完成瞬时提示（processed == total，只弹一次）。
            if green {
                if !app.filter_completed_toasted {
                    app.filter_completed_toasted = true;
                    let deleted = app.state.ws.counts.deleted;
                    let kept = total - deleted;
                    let msg = match crate::i18n::lang() {
                        crate::i18n::Lang::Zh => {
                            format!("筛选完成！共 {total} 张，保留 {kept} 张，待删 {deleted} 张")
                        }
                        crate::i18n::Lang::En => format!(
                            "Culling complete! {total} total, {kept} kept, {deleted} to delete"
                        ),
                    };
                    app.toast(ToastKind::Success, msg);
                }
            } else {
                app.filter_completed_toasted = false;
            }
            let color = if green { theme::KEEP } else { theme::ACCENT };
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    fill.min,
                    egui::vec2(fill.width() * frac, fill.height()),
                ),
                0.0,
                color,
            );
        });

    // ---- Status bar (bottom, outermost) ----
    egui::Panel::bottom("status_bar")
        .default_size(theme::STATUS_BAR_HEIGHT)
        .size_range(egui::Rangef::new(theme::STATUS_BAR_HEIGHT, theme::STATUS_BAR_HEIGHT))
        .frame(
            egui::Frame::default()
                .fill(theme::STATUS_BAR_BG)
                .inner_margin(egui::Margin {
                    left: 14,
                    right: 20,
                    top: 0,
                    bottom: 0,
                })
                .outer_margin(egui::Margin::ZERO),
        )
        .show(ui, |ui| render_status_bar(app, ui));

    // ---- Thumbnail strip (bottom above status bar) ----
    egui::Panel::bottom("thumb_strip")
        .resizable(true)
        .default_size(theme::THUMB_STRIP_DEFAULT_H)
        .size_range(egui::Rangef::new(80.0, 300.0))
        .frame(frame_pad(theme::THUMB_STRIP_BG, 20, 10))
        .show(ui, |ui| {
            if app.state.ws.items.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new(t("暂无照片", "No photos")).color(theme::TEXT_WEAK));
                });
            } else {
                render_thumb_strip(app, ui);
            }
        });

    // ---- Right info panel (only when a workspace is open) ----
    if has_ws && app.state.right_panel_visible {
        egui::Panel::right("info_panel")
            .resizable(true)
            .default_size(app.state.right_panel_width)
            .size_range(egui::Rangef::new(200.0, 500.0))
            .frame(frame_fill(theme::RIGHT_PANEL_BG))
            .show(ui, |ui| {
                app.state.right_panel_width = ui.available_width();
                draw_right_panel(app, ui);
            });
    }

    // ---- Central preview ----
    egui::CentralPanel::default()
        .frame(frame_fill(theme::PREVIEW_BG))
        .show(ui, |ui| {
            if !has_ws {
                render_empty_state(app, ui);
            } else {
                render_preview(app, ui);
            }
        });
}

fn frame_fill(fill: egui::Color32) -> egui::Frame {
    egui::Frame::default()
        .fill(fill)
        .inner_margin(egui::Margin::ZERO)
        .outer_margin(egui::Margin::ZERO)
}

fn frame_pad(fill: egui::Color32, h: i8, v: i8) -> egui::Frame {
    egui::Frame::default()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(h, v))
        .outer_margin(egui::Margin::ZERO)
}

fn truncate_path(path: &str, max: usize) -> String {
    if path.chars().count() <= max || path.is_empty() {
        return path.to_string();
    }
    // Keep the tail (folder name) and trim the head.
    let tail_len = max - 3;
    let tail: String = path.chars().rev().take(tail_len).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{tail}")
}

fn sort_dropdown(app: &mut KakaApp, ui: &mut egui::Ui) {
    let current = app.state.ws.sort;
    egui::ComboBox::from_id_salt("sort")
        .selected_text(format!("☰ {}: {}", t("排序", "Sort"), current.label()))
        .width(150.0)
        .show_ui(ui, |ui| {
            for so in [
                SortOrder::CaptureTimeAsc,
                SortOrder::CaptureTimeDesc,
                SortOrder::FilenameAsc,
                SortOrder::FilenameDesc,
                SortOrder::FileSizeAsc,
                SortOrder::FileSizeDesc,
                SortOrder::ImportTimeAsc,
                SortOrder::ImportTimeDesc,
                SortOrder::StatusGrouped,
            ] {
                let selected = current == so;
                if ui.selectable_label(selected, so.label()).clicked() {
                    app.state.ws.sort = so;
                    let _ = app.state.reload_current();
                }
            }
        });
}

/// Apply the simple filename search filter (composes with the advanced filter).
fn apply_search(app: &mut KakaApp, needle: &str) {
    if app.state.ws.folder_path.is_empty() {
        return;
    }
    app.state.ws.search = needle.to_string();
    app.state.ws.current_index = 0;
    let _ = app.state.apply_view();
}

fn render_status_bar(app: &mut KakaApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let gear = egui::Button::new(RichText::new("⚙").size(18.0).color(theme::TEXT_SECONDARY));
        if ui.add(gear).on_hover_text(t("设置", "Settings")).clicked() {
            app.open_settings();
        }
        ui.separator();

        let counts = &app.state.ws.counts;
        let processed = counts.deleted + counts.reviewed;
        let total = counts.total;
        let green = total > 0 && processed >= total;
        let frac_color = if green { theme::KEEP } else { theme::TEXT };
        ui.label(
            RichText::new(format!("{} {processed} / {total}", t("已筛选", "Processed")))
                .size(14.0)
                .color(frac_color)
                .strong(),
        );
        sep(ui);
        ui.label(RichText::new(format!("{} {}", t("保留", "Keep"), total - counts.deleted)).size(14.0).color(theme::KEEP));
        sep(ui);
        ui.label(RichText::new(format!("{} {}", t("已阅", "Reviewed"), counts.reviewed)).size(14.0).color(theme::TEXT_SECONDARY));
        sep(ui);
        ui.label(RichText::new(format!("{} {}", t("待删", "Delete"), counts.deleted)).size(14.0).color(theme::DELETE));
        if app.state.ws.selected_count() > 0 {
            sep(ui);
            ui.label(RichText::new(format!("{} {}", t("选中", "Selected"), app.state.ws.selected_count())).size(14.0).color(theme::ACCENT));
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // 选中含丢失记录时的操作提示 (PRD 7.9.2).
            let missing_in_sel = app
                .state
                .ws
                .items
                .iter()
                .filter(|p| p.is_missing() && app.state.ws.selection.contains(&p.id))
                .count();
            if missing_in_sel > 0 {
                ui.label(
                    RichText::new(match crate::i18n::lang() {
                        crate::i18n::Lang::Zh => format!(
                            "选中含 {missing_in_sel} 条丢失记录 · 右键预览可移除"
                        ),
                        crate::i18n::Lang::En => format!(
                            "{missing_in_sel} missing in selection · right-click preview to remove"
                        ),
                    })
                    .size(13.0)
                    .color(theme::DELETE),
                );
            }
            // Hint reflects the user's current bindings (PRD 7.6).
            let kb = &app.state.config.keybindings;
            let disp = |action: &str| {
                crate::app::keybinds::display(&crate::app::keybinds::effective_codes(kb, action)[0])
            };
            ui.label(
                RichText::new(format!(
                    "{} {} | {} {} | {} {}",
                    disp("mark_delete"),
                    t("待删", "delete"),
                    disp("mark_reviewed"),
                    t("跳过", "skip"),
                    disp("next_photo"),
                    t("下一张", "next"),
                ))
                .size(13.0)
                .color(theme::TEXT_WEAK),
            );
        });
    });
}

fn sep(ui: &mut egui::Ui) {
    ui.label(RichText::new("|").color(theme::BORDER_2));
}

/// Draw a texture rotated by `turns` × 90° clockwise about `center` (PRD 7.2).
/// `size` is the UNROTATED drawn size; for 90/270 turns the visible AABB has
/// swapped axes, so callers compute fit/clamping against the swapped dims and
/// anchor overlays to that AABB. `turns % 4 == 0` degrades to a plain
/// axis-aligned image draw. Also used by the delete-box grid (PRD 8.1).
pub(super) fn draw_image_rotated(
    painter: &egui::Painter,
    tex_id: egui::TextureId,
    center: egui::Pos2,
    size: egui::Vec2,
    turns: i64,
    tint: egui::Color32,
) {
    let turns = turns.rem_euclid(4);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if turns == 0 {
        painter.image(
            tex_id,
            egui::Rect::from_center_size(center, size),
            uv,
            tint,
        );
        return;
    }
    // Screen space is y-down, so this matrix rotates the image clockwise.
    let angle = turns as f32 * std::f32::consts::FRAC_PI_2;
    let (sin, cos) = angle.sin_cos();
    let corners = [
        egui::vec2(-size.x * 0.5, -size.y * 0.5),
        egui::vec2(size.x * 0.5, -size.y * 0.5),
        egui::vec2(size.x * 0.5, size.y * 0.5),
        egui::vec2(-size.x * 0.5, size.y * 0.5),
    ];
    let uvs = [
        egui::pos2(0.0, 0.0),
        egui::pos2(1.0, 0.0),
        egui::pos2(1.0, 1.0),
        egui::pos2(0.0, 1.0),
    ];
    let mut mesh = egui::Mesh::default();
    // Mesh::default() binds TextureId::Managed(0) — the font atlas. A raw mesh
    // must set its texture explicitly or it renders atlas garbage.
    mesh.texture_id = tex_id;
    let base = mesh.vertices.len() as u32;
    for (c, uv) in corners.iter().zip(uvs) {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: center + egui::vec2(c.x * cos - c.y * sin, c.x * sin + c.y * cos),
            uv,
            color: egui::Color32::WHITE,
        });
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    painter.add(egui::Shape::mesh(mesh));
}

fn render_thumb_strip(app: &mut KakaApp, ui: &mut egui::Ui) {
    let current_id = app.state.ws.current().map(|p| p.id);
    let items_len = app.state.ws.items.len();
    let should_center = app.last_centered_id != current_id;
    let mut clicked = None;
    // Modifiers for the current frame (Ctrl/Shift for multi-select, PRD 7.9.1).
    let mods = ui.input(|i| i.modifiers);

    egui::ScrollArea::horizontal()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for idx in 0..items_len {
                    let item = &app.state.ws.items[idx];
                    let is_current = Some(item.id) == current_id;
                    let is_selected = app.state.ws.selection.contains(&item.id);
                    // Non-blocking: read cache only; enqueue a background job if missing.
                    let (tex, needs) = app.textures.texture_for(ui.ctx(), item);
                    if needs {
                        let hash = item.thumb_hash.clone().unwrap_or_default();
                        app.thumbs.enqueue(item.id, &hash, &item.current_path);
                    }
                    let dim_reviewed =
                        app.state.config.dim_reviewed_thumbnails && item.status == Status::Reviewed;
                    let (clicked_item, rect) =
                        thumb_widget(ui, &tex, item, is_current, is_selected, dim_reviewed);
                    if is_current && should_center {
                        ui.scroll_to_rect(rect, Some(egui::Align::Center));
                    }
                    if clicked_item {
                        clicked = Some(idx);
                    }
                }
            });
        });

    if should_center {
        app.last_centered_id = current_id;
    }
    if let Some(idx) = clicked {
        app.state.select_click(idx, mods.ctrl, mods.shift);
        app.needs_save = true;
        app.last_centered_id = current_id;
    }
}

/// Draw a single thumbnail tile, returning (clicked, tile_rect).
fn thumb_widget(
    ui: &mut egui::Ui,
    tex: &egui::TextureHandle,
    item: &PhotoListItem,
    is_current: bool,
    is_selected: bool,
    dim_reviewed: bool,
) -> (bool, egui::Rect) {
    let size = egui::vec2(110.0, 76.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter();

    // Canvas (image) area: 100 x 66 centered within the frame.
    let img_h = 66.0;
    let img_w = 100.0;
    let offset = egui::vec2((size.x - img_w) / 2.0, (size.y - img_h) / 2.0);
    let img_rect = egui::Rect::from_min_size(rect.min + offset, egui::vec2(img_w, img_h));
    painter.rect_filled(img_rect, 0.0, theme::PREVIEW_BG);

    let ts = tex.size_vec2();
    if ts.x > 0.0 && ts.y > 0.0 {
        // Sync the manual rotation into the strip (PRD 4.7): for 90/270 turns
        // the fit box swaps axes so the rotated image still fills the canvas.
        let turns = item.rotation_override.rem_euclid(4);
        let swapped = turns % 2 == 1;
        let (bw, bh) = if swapped {
            (img_rect.height(), img_rect.width())
        } else {
            (img_rect.width(), img_rect.height())
        };
        let scale = (bw / ts.x).min(bh / ts.y);
        let size = egui::vec2(ts.x * scale, ts.y * scale);
        // 淡化已阅跳过 (PRD 4.2 / UI spec 3.5-4): mute reviewed thumbnails with
        // a gray tint when the setting is on.
        let tint = if dim_reviewed {
            egui::Color32::from_rgb(0xc4, 0xc4, 0xc4)
        } else {
            egui::Color32::WHITE
        };
        if turns == 0 {
            let draw_rect = egui::Rect::from_center_size(img_rect.center(), size);
            painter.image(
                tex.id(),
                draw_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                tint,
            );
        } else {
            draw_image_rotated(painter, tex.id(), img_rect.center(), size, turns, tint);
        }
    }

    // Border: current (focus) = 3px accent; selected = 2px blue; else 1px border.
    let stroke = if is_current {
        egui::Stroke::new(3.0, theme::ACCENT)
    } else if is_selected {
        egui::Stroke::new(2.0, theme::BLUE)
    } else {
        egui::Stroke::new(1.0, theme::BORDER)
    };
    painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);

    // Status badges (top-right): 待删 red or 已阅 green.
    match item.status {
        Status::Delete => {
            let badge = egui::Rect::from_min_size(
                egui::pos2(rect.max.x - 34.0, rect.min.y),
                egui::vec2(34.0, 20.0),
            );
            painter.rect_filled(badge, 0.0, theme::DELETE);
            painter.text(
                badge.center(),
                Align2::CENTER_CENTER,
                t("待删", "Delete"),
                egui::FontId::proportional(10.0),
                egui::Color32::WHITE,
            );
        }
        Status::Reviewed => {
            let badge = egui::Rect::from_min_size(
                egui::pos2(rect.max.x - 34.0, rect.min.y),
                egui::vec2(34.0, 20.0),
            );
            painter.rect_filled(badge, 0.0, theme::KEEP);
            painter.text(
                badge.center(),
                Align2::CENTER_CENTER,
                t("已阅", "Reviewed"),
                egui::FontId::proportional(10.0),
                egui::Color32::from_rgb(0x0f, 0x2a, 0x1c),
            );
        }
        Status::Untreated => {}
    }
    // R+J 角标始终显示（含待删状态），这样整组标记后仍能看出它属于配对组。
    if item.pair_group_id.is_some() {
        painter.text(
            egui::pos2(rect.max.x - 28.0, rect.max.y - 4.0),
            Align2::RIGHT_BOTTOM,
            "R+J",
            egui::FontId::proportional(10.0),
            theme::TEXT,
        );
    }

    (resp.clicked(), rect)
}

fn render_preview(app: &mut KakaApp, ui: &mut egui::Ui) {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    // Owned clone: the minimap below allocates widgets through `ui` while the
    // painter keeps drawing into the same clip rect.
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 0.0, theme::PREVIEW_BG);

    let Some(item) = app.state.ws.current().cloned() else {
        return;
    };
    // Reset zoom when the current photo changes (anchor is per photo, PRD 7.4).
    if app.zoom_photo_id != Some(item.id) {
        app.zoom_active = false;
        app.zoom_photo_id = Some(item.id);
        app.missing_overlay_dismissed = None;
    }

    // 文件丢失 (PRD 7.9.2 / UI 3.3.1): dark overlay + 移除丢失记录/稍后处理.
    // The normal texture pipeline is skipped entirely (the file is gone —
    // generating would spin the thumbnail worker forever).
    if item.is_missing() && app.missing_overlay_dismissed != Some(item.id) {
        // 磁盘缓存的预览图仍保留（移除记录只清登记），画出它的"遗像"并压上
        // 半透明覆盖层。绝不入队生成——源文件已不存在。
        let (tex, _needs) = app.textures.preview_for(ui.ctx(), &item);
        draw_missing_overlay(app, ui, rect, &item, &tex);
        preview_context_menu(app, ui, &resp, &item);
        return;
    }

    let (tex, needs) = app.textures.preview_for(ui.ctx(), &item);
    if needs {
        let hash = item.thumb_hash.clone().unwrap_or_default();
        app.thumbs.enqueue(item.id, &hash, &item.current_path);
    }
    let ts = tex.size_vec2();
    // 视口小地图 (PRD 4.6): set in the 100% branch, painted last so it sits
    // above photo and badges. (tex_id, image AABB on screen, map box, turns)
    let mut map_hud: Option<(egui::TextureId, egui::Rect, egui::Rect, i64)> = None;
    if ts.x > 0.0 && ts.y > 0.0 {
        let draw_rect;
        if app.zoom_active {
            // 100%: 1 RAW pixel = 1 screen pixel (PRD 7.4). Until the
            // background full-resolution decode lands, the embedded preview is
            // stretched into the RAW's true dimensions, so the framing is
            // already correct and the swap to sharp pixels is seamless.
            let full_tex = app.zoom_texture(&item);
            let raw_dims = full_tex
                .as_ref()
                .map(|t| t.size_vec2())
                .or_else(|| {
                    app.zoom_dims
                        .get(&item.id)
                        .map(|(w, h)| egui::vec2(*w as f32, *h as f32))
                })
                .unwrap_or(ts);
            // True 100% (PRD 7.4): 1 image pixel = 1 PHYSICAL screen pixel.
            // egui works in logical points and multiplies by pixels_per_point
            // on render, so divide — otherwise a 2K screen at 125/150% scaling
            // silently magnifies the image and 100% differs between screens.
            let ppp = ui.ctx().pixels_per_point();
            let dims = raw_dims / ppp;

            // Pan with Ctrl+drag (PRD 7.4 / M3 更正). The anchor is the image
            // point (fractions 0..1) at the viewport center, so it survives
            // the preview -> RAW texture swap unchanged. Pan math uses the
            // rotated AABB (PRD 7.2) so dragging stays screen-natural.
            let turns = item.rotation_override.rem_euclid(4);
            let swapped = turns % 2 == 1;
            let rdims = if swapped { egui::vec2(dims.y, dims.x) } else { dims };
            // Effective on-screen AABB = 1:1 baseline × animated zoom scale.
            // The wheel updates TARGET values only; the displayed values ease
            // toward them below, so each detent (and the clamp/fit handoffs)
            // glides instead of jumping. Computed after the easing step: the
            // wheel anchor, pan sensitivity and the clamp all must work in the
            // same scaled space.
            //
            // Ctrl+滚轮自由缩放（PRD 7.4 补充）: geometric stepping like the
            // Windows Photo Viewer — each wheel detent multiplies or divides
            // the zoom by exactly 1.1, fine-grained at low zoom and responsive
            // at high zoom. Cursor-anchored; range 10%..800%. Wheel deltas
            // arrive in different units per device (lines / points / pages)
            // and are normalized to detents first.
            let notches = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::MouseWheel { unit, delta, modifiers, .. } if modifiers.ctrl => {
                            let y = delta.y;
                            Some(match unit {
                                egui::MouseWheelUnit::Line => y,
                                egui::MouseWheelUnit::Point => y / (53.0 * ppp),
                                egui::MouseWheelUnit::Page => y * 3.0,
                            })
                        }
                        _ => None,
                    })
                    .sum::<f32>()
            });
            if notches != 0.0 && resp.hovered() {
                let old_eff = rdims * app.zoom_scale_target;
                app.zoom_scale_target =
                    (app.zoom_scale_target * 1.1f32.powf(notches)).clamp(0.1, 8.0);
                let new_eff = rdims * app.zoom_scale_target;
                // Cursor-anchored: keep the image point under the cursor fixed
                // (anchor math runs entirely in target space).
                if let Some(pos) = resp.hover_pos() {
                    let off = pos - rect.center();
                    let under = egui::vec2(off.x / old_eff.x, off.y / old_eff.y)
                        + egui::vec2(app.zoom_center_target.0, app.zoom_center_target.1);
                    app.zoom_center_target.0 = (under.x - off.x / new_eff.x).clamp(0.0, 1.0);
                    app.zoom_center_target.1 = (under.y - off.y / new_eff.y).clamp(0.0, 1.0);
                }
            }

            // Ease the displayed state toward the targets (exponential
            // smoothing, ~60ms half-life): wheel detents, the pan-clamp
            // handoff and the fit-crossing all become short glides. Pan
            // writes the displayed value directly and syncs the target, so
            // dragging stays 1:1 with no easing lag.
            let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.1);
            let k = 1.0 - (-dt / 0.05f32).exp();
            app.zoom_scale += (app.zoom_scale_target - app.zoom_scale) * k;
            app.zoom_center.0 += (app.zoom_center_target.0 - app.zoom_center.0) * k;
            app.zoom_center.1 += (app.zoom_center_target.1 - app.zoom_center.1) * k;
            let settled = (app.zoom_scale - app.zoom_scale_target).abs() < 0.0005
                && (app.zoom_center.0 - app.zoom_center_target.0).abs() < 0.0005
                && (app.zoom_center.1 - app.zoom_center_target.1).abs() < 0.0005;
            if !settled {
                ui.ctx().request_repaint();
            }
            let rdims_eff = rdims * app.zoom_scale;

            // 视口小地图 (PRD 4.6 / UI 3.3.1): bottom-right overview; click or
            // drag jumps the viewport. The rect is stored and painted after
            // the badges. Only when the image overflows the viewport.
            let map_fit = egui::vec2(96.0, 60.0); // PRD: < 100x60
            let aspect = rdims.x / rdims.y.max(1.0);
            let (mw, mh) = if aspect >= map_fit.x / map_fit.y {
                (map_fit.x, map_fit.x / aspect)
            } else {
                (map_fit.y * aspect, map_fit.y)
            };
            let map_rect = egui::Rect::from_min_size(
                egui::pos2(rect.max.x - 12.0 - mw, rect.max.y - 12.0 - mh),
                egui::vec2(mw, mh),
            );
            let map_resp = ui.allocate_rect(map_rect, egui::Sense::click_and_drag());
            let map_hit = map_resp.clicked() || map_resp.dragged();
            if map_hit {
                if let Some(pos) = map_resp.interact_pointer_pos() {
                    // Target only: the view glides to the clicked spot.
                    app.zoom_center_target.0 =
                        ((pos.x - map_rect.min.x) / map_rect.width()).clamp(0.0, 1.0);
                    app.zoom_center_target.1 =
                        ((pos.y - map_rect.min.y) / map_rect.height()).clamp(0.0, 1.0);
                }
            }

            if ui.input(|i| i.modifiers.ctrl) && resp.dragged() && !map_resp.dragged() {
                // Divide by the SCALED size so the image tracks the cursor
                // 1:1 at any zoom level (was stuck at the 1:1 baseline, which
                // flung the view to the edges at high magnification). Pan is
                // immediate: displayed and target move together.
                let d = resp.drag_delta();
                let nx = (app.zoom_center.0 - d.x / rdims_eff.x.max(1.0)).clamp(0.0, 1.0);
                let ny = (app.zoom_center.1 - d.y / rdims_eff.y.max(1.0)).clamp(0.0, 1.0);
                app.zoom_center = (nx, ny);
                app.zoom_center_target = (nx, ny);
            }
            let mut cx = app.zoom_center.0;
            let mut cy = app.zoom_center.1;
            // Keep the image covering the viewport when it is larger; center
            // otherwise (PRD 7.4 平移约束). Must clamp against the SCALED size
            // (rdims_eff) — clamping to the 1:1 baseline would pin the pan
            // range to the 100% window and leave outer regions unreachable
            // after Ctrl+滚轮.
            cx = if rdims_eff.x > rect.width() {
                cx.clamp(
                    rect.width() * 0.5 / rdims_eff.x,
                    1.0 - rect.width() * 0.5 / rdims_eff.x,
                )
            } else {
                0.5
            };
            cy = if rdims_eff.y > rect.height() {
                cy.clamp(
                    rect.height() * 0.5 / rdims_eff.y,
                    1.0 - rect.height() * 0.5 / rdims_eff.y,
                )
            } else {
                0.5
            };
            app.zoom_center = (cx, cy);
            app.zoom_anchors.insert(item.id, (cx, cy));

            let top_left = egui::pos2(
                rect.center().x - cx * rdims_eff.x,
                rect.center().y - cy * rdims_eff.y,
            );
            draw_rect = egui::Rect::from_min_size(top_left, rdims_eff);
            // When zoomed out far enough that the RAW texture displays below
            // the preview's own resolution, draw the PREVIEW texture instead:
            // it is the higher-quality rendition at that size (one Lanczos
            // downscale vs a 5×+ mipmap chain, which washes out fine grain
            // like beach sand) and is pixel-identical to the fit view — so
            // zooming out never shifts the look (LR-like). Past the
            // threshold the full-resolution pixels carry real extra detail.
            let drawn_device_w = raw_dims.x * app.zoom_scale;
            let shown = match full_tex.as_ref() {
                Some(ft) if drawn_device_w > ts.x => ft,
                _ => &tex,
            };
            draw_image_rotated(
                &painter,
                shown.id(),
                draw_rect.center(),
                dims * app.zoom_scale,
                turns,
                egui::Color32::WHITE,
            );
            if rdims_eff.x > rect.width() || rdims_eff.y > rect.height() {
                map_hud = Some((shown.id(), draw_rect, map_rect, turns));
            }

            // Zoom status label (PRD 4.6): current magnification, with
            // RAW-specific hints only for RAW files.
            let is_raw = crate::io::format::is_raw(std::path::Path::new(&item.current_path));
            let pct = (app.zoom_scale * 100.0).round() as i64;
            let status = if !is_raw {
                format!("{pct}%")
            } else {
                let note = if full_tex.is_some() {
                    t("RAW 原生像素", "RAW pixels")
                } else if app.zoom_worker.is_pending(item.id) {
                    t("RAW 解码中…（先以内嵌预览显示）", "decoding RAW… (embedded preview)")
                } else if item.decode_failed {
                    t(
                        "RAW 解码失败，显示内嵌预览（右键可重试）",
                        "RAW decode failed — embedded preview (right-click to retry)",
                    )
                } else {
                    ""
                };
                if note.is_empty() {
                    format!("{pct}%")
                } else {
                    format!("{pct}% · {note}")
                }
            };
            painter.text(
                egui::pos2(rect.min.x + 8.0, rect.min.y + 8.0),
                Align2::LEFT_TOP,
                status,
                egui::FontId::proportional(12.0),
                theme::ACCENT,
            );
        } else {
            let margin = 24.0;
            let avail = egui::vec2(rect.width() - margin * 2.0, rect.height() - margin * 2.0)
                .max(egui::vec2(1.0, 1.0));
            // Manual rotation (PRD 7.2) stacks on top of the EXIF-oriented
            // texture: for 90/270 turns the fit box and the visible AABB swap,
            // and badges anchor to the AABB.
            let turns = item.rotation_override.rem_euclid(4);
            let swapped = turns % 2 == 1;
            let (tw, th) = if swapped { (ts.y, ts.x) } else { (ts.x, ts.y) };
            let scale = (avail.x / tw).min(avail.y / th);
            let size = egui::vec2(tw * scale, th * scale);
            draw_rect = egui::Rect::from_center_size(rect.center(), size);
            draw_image_rotated(
                &painter,
                tex.id(),
                rect.center(),
                egui::vec2(ts.x * scale, ts.y * scale),
                turns,
                egui::Color32::WHITE,
            );
        }

        // Status corner badge.
        match item.status {
            Status::Delete => {
                let badge = egui::Rect::from_min_size(
                    egui::pos2(draw_rect.max.x - 42.0, draw_rect.min.y),
                    egui::vec2(42.0, 24.0),
                );
                painter.rect_filled(badge, 0.0, theme::DELETE);
                painter.text(
                    badge.center(),
                    Align2::CENTER_CENTER,
                    t("待删", "Delete"),
                    egui::FontId::proportional(12.0),
                    egui::Color32::WHITE,
                );
            }
            Status::Reviewed => {
                let badge = egui::Rect::from_min_size(
                    egui::pos2(draw_rect.max.x - 42.0, draw_rect.min.y),
                    egui::vec2(42.0, 24.0),
                );
                painter.rect_filled(badge, 0.0, theme::KEEP);
                painter.text(
                    badge.center(),
                    Align2::CENTER_CENTER,
                    t("已阅", "Reviewed"),
                    egui::FontId::proportional(12.0),
                    egui::Color32::from_rgb(0x0f, 0x2a, 0x1c),
                );
            }
            Status::Untreated => {}
        }
    }

    // 视口小地图 (PRD 4.6): painted last so it sits above photo and badges.
    if let Some((tex_id, draw_rect, map_rect, turns)) = map_hud {
        let visible = rect.intersect(draw_rect);
        draw_viewport_minimap(&painter, tex_id, draw_rect, visible, map_rect, turns);
    }

    // Right-click menu on the preview (PRD 3.3.2, core subset).
    preview_context_menu(app, ui, &resp, &item);
}

/// 视口小地图 (PRD 4.6 / UI 3.3.1): a rotated overview in the bottom-right
/// corner. The visible viewport region is drawn bright while the rest stays
/// under a semi-transparent mask; click or drag jumps the viewport (the
/// interaction itself is handled in `render_preview` before the pan logic).
fn draw_viewport_minimap(
    painter: &egui::Painter,
    tex_id: egui::TextureId,
    draw_rect: egui::Rect,
    visible: egui::Rect,
    map_rect: egui::Rect,
    turns: i64,
) {
    // The on-screen AABB already includes the free-zoom scale.
    let rdims = draw_rect.size();
    // Overview image, rotated exactly like the main view and aspect-fitted so
    // its AABB equals the map box.
    let map_img_size = if turns % 2 == 1 {
        egui::vec2(map_rect.height(), map_rect.width())
    } else {
        map_rect.size()
    };
    draw_image_rotated(painter, tex_id, map_rect.center(), map_img_size, turns, egui::Color32::WHITE);

    // Mask everything, then re-draw the visible region bright (clipped).
    painter.rect_filled(map_rect, 0.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 0x88));

    let fx = |x: f32| ((x - draw_rect.min.x) / rdims.x.max(1.0)).clamp(0.0, 1.0);
    let fy = |y: f32| ((y - draw_rect.min.y) / rdims.y.max(1.0)).clamp(0.0, 1.0);
    let p0 = egui::pos2(
        map_rect.min.x + fx(visible.min.x) * map_rect.width(),
        map_rect.min.y + fy(visible.min.y) * map_rect.height(),
    );
    let p1 = egui::pos2(
        map_rect.min.x + fx(visible.max.x) * map_rect.width(),
        map_rect.min.y + fy(visible.max.y) * map_rect.height(),
    );
    let sub = egui::Rect::from_min_max(p0, p1);

    let clipped = painter.with_clip_rect(sub);
    draw_image_rotated(&clipped, tex_id, map_rect.center(), map_img_size, turns, egui::Color32::WHITE);

    painter.rect_stroke(sub, 0.0, egui::Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
    painter.rect_stroke(map_rect, 0.0, egui::Stroke::new(1.0, theme::BORDER_2), egui::StrokeKind::Inside);
}

/// Right-click menu for the preview area (PRD 3.3.2): marking, RAW retry,
/// reveal in Explorer, copy path.
/// 文件丢失覆盖层 (PRD 7.9.2 / UI 3.3.1): semi-transparent dark cover with a
/// title, the dead path, and 移除丢失记录 / 稍后处理 buttons.
fn draw_missing_overlay(
    app: &mut KakaApp,
    ui: &mut egui::Ui,
    rect: egui::Rect,
    item: &PhotoListItem,
    tex: &egui::TextureHandle,
) {
    // 遗像：磁盘缓存的预览图按适配绘制，覆盖层压在其上仍可辨认。
    let ts = tex.size_vec2();
    if ts.x > 0.0 && ts.y > 0.0 {
        let avail = rect.shrink(24.0);
        let scale = (avail.width() / ts.x).min(avail.height() / ts.y).min(1.0);
        let img_rect = egui::Rect::from_center_size(rect.center(), ts * scale);
        ui.painter().image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
    ui.painter()
        .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(130));
    let c = rect.center();
    ui.painter().text(
        egui::pos2(c.x, c.y - 64.0),
        egui::Align2::CENTER_CENTER,
        t("文件已丢失", "File missing"),
        egui::FontId::proportional(24.0),
        theme::DELETE,
    );
    ui.painter().text(
        egui::pos2(c.x, c.y - 30.0),
        egui::Align2::CENTER_CENTER,
        t("数据库中保留着这条记录，但磁盘上的文件已不存在：", "The library keeps this record, but the file on disk no longer exists:"),
        egui::FontId::proportional(13.0),
        theme::TEXT_SECONDARY,
    );
    ui.painter().text(
        egui::pos2(c.x, c.y - 10.0),
        egui::Align2::CENTER_CENTER,
        &item.current_path,
        egui::FontId::proportional(12.0),
        theme::TEXT_WEAK,
    );

    let bw = 160.0;
    let bh = 32.0;
    let y = c.y + 30.0;
    let remove_rect =
        egui::Rect::from_center_size(egui::pos2(c.x - bw / 2.0 - 10.0, y), egui::vec2(bw, bh));
    let later_rect =
        egui::Rect::from_center_size(egui::pos2(c.x + bw / 2.0 + 10.0, y), egui::vec2(bw, bh));

    let remove_btn = egui::Button::new(
        RichText::new(t("移除丢失记录", "Remove record"))
            .size(14.0)
            .color(egui::Color32::WHITE),
    )
    .fill(theme::DELETE)
    .stroke(egui::Stroke::new(1.0, theme::DELETE));
    if ui.put(remove_rect, remove_btn).clicked() {
        let id = item.id;
        app.remove_missing_record(id);
        app.toast(
            ToastKind::Info,
            t("已移除丢失记录（仅数据库，不动磁盘文件）", "Missing record removed (library only, no files touched)"),
        );
    }
    if ui
        .put(later_rect, egui::Button::new(RichText::new(t("稍后处理", "Later")).size(14.0)))
        .clicked()
    {
        app.missing_overlay_dismissed = Some(item.id);
    }
}

fn preview_context_menu(
    app: &mut KakaApp,
    _ui: &mut egui::Ui,
    resp: &egui::Response,
    item: &PhotoListItem,
) {
    resp.context_menu(|ui| {
        // 文件丢失条目 (PRD 7.9.2 / UI 3.3.2-12).
        if item.is_missing()
            && ui
                .button(t("从数据库移除此记录", "Remove this record from the library"))
                .clicked()
        {
            let id = item.id;
            app.remove_missing_record(id);
            app.toast(
                ToastKind::Info,
                t("已移除丢失记录（仅数据库，不动磁盘文件）", "Missing record removed (library only, no files touched)"),
            );
        }
        // 批量移除选区内的丢失记录 (PRD 7.9.2) — needs 二次确认 per setting.
        let missing_in_sel = app
            .state
            .ws
            .items
            .iter()
            .filter(|p| p.is_missing() && app.state.ws.selection.contains(&p.id))
            .count();
        if missing_in_sel > 0
            && ui
                .button(format!(
                    "{}（{missing_in_sel}）",
                    t("移除选区内的丢失记录", "Remove missing records in selection")
                ))
                .clicked()
        {
            if app.state.config.batch_confirm {
                app.confirm = Some(ConfirmDialog {
                    title: t("移除丢失记录", "Remove missing records").into(),
                    text: match crate::i18n::lang() {
                        crate::i18n::Lang::Zh => format!(
                            "将把选区中的 {missing_in_sel} 条丢失记录从数据库移除。
只删除数据库记录，不会动磁盘上的任何文件。确定？"
                        ),
                        crate::i18n::Lang::En => format!(
                            "{missing_in_sel} missing records in the selection will be removed from the library.
Only database rows are deleted — no files are touched. Continue?"
                        ),
                    },
                    confirm_label: t("移除", "Remove").into(),
                    danger: true,
                    on_confirm: Box::new(|app| app.remove_missing_in_selection()),
                });
            } else {
                app.remove_missing_in_selection();
            }
        }
        if item.is_missing() || missing_in_sel > 0 {
            ui.separator();
        }
        if ui.button(t("标记待删（Q）", "Mark for deletion (Q)")).clicked() {
            let _ = app.state.set_status_current(Status::Delete, true);
            app.advance(1);
            app.needs_save = true;
        }
        if ui.button(t("标记已阅跳过（E）", "Mark reviewed / skip (E)")).clicked() {
            let _ = app.state.set_status_current(Status::Reviewed, true);
            app.advance(1);
            app.needs_save = true;
        }
        if ui.button(t("重置为未处理（U）", "Reset to unprocessed (U)")).clicked() {
            let _ = app.state.set_status_current(Status::Untreated, true);
            app.needs_save = true;
        }
        ui.separator();
        // Rotation (PRD 3.3.2 / 7.2): DB-only display rotation.
        if ui.button(t("顺时针旋转 90°（R）", "Rotate 90° CW (R)")).clicked() {
            let _ = app.state.rotate_current(1);
            app.needs_save = true;
            app.toast(ToastKind::Info, t("已顺时针旋转 90°", "Rotated 90° clockwise"));
        }
        if ui.button(t("逆时针旋转 90°（Ctrl+R）", "Rotate 90° CCW (Ctrl+R)")).clicked() {
            let _ = app.state.rotate_current(-1);
            app.needs_save = true;
            app.toast(ToastKind::Info, t("已逆时针旋转 90°", "Rotated 90° counter-clockwise"));
        }
        if ui.button(t("重置旋转（Shift+R）", "Reset rotation (Shift+R)")).clicked() {
            let _ = app.state.rotate_current(0);
            app.needs_save = true;
            app.toast(ToastKind::Info, t("已重置为 EXIF 方向", "Reset to EXIF orientation"));
        }
        ui.separator();
        if item.decode_failed && ui.button(t("强制重试 RAW 解码", "Force RAW decode retry")).clicked() {
            app.retry_zoom_decode(item.id);
        }
        if ui.button(t("在资源管理器中显示", "Show in Explorer")).clicked() {
            let _ = std::process::Command::new("explorer")
                .arg(format!("/select,{}", item.current_path))
                .spawn();
        }
        if ui.button(t("复制文件路径", "Copy file path")).clicked() {
            ui.ctx().copy_text(item.current_path.clone());
        }
    });
}

fn draw_right_panel(app: &mut KakaApp, ui: &mut egui::Ui) {
    ui.add_space(8.0);
    ui.label(RichText::new(t("信息面板", "Photo info")).size(12.0).color(theme::TEXT_SECONDARY).strong());
    ui.separator();
    match app.state.ws.current().cloned() {
        Some(p) => {
            let rows: Vec<(&str, String)> = vec![
                (t("相机", "Camera"), p.camera_model.clone().unwrap_or_else(|| "—".into())),
                (t("镜头", "Lens"), p.lens_model.clone().unwrap_or_else(|| "—".into())),
                (t("焦距", "Focal length"), p.focal_length.map(|v| format!("{v} mm")).unwrap_or_else(|| "—".into())),
                (t("光圈", "Aperture"), p.aperture.clone().unwrap_or_else(|| "—".into())),
                (t("快门", "Shutter"), p.shutter_speed.clone().unwrap_or_else(|| "—".into())),
                ("ISO", p.iso.map(|v| v.to_string()).unwrap_or_else(|| "—".into())),
                (t("时间", "Taken"), p.capture_time.clone()),
                (t("大小", "Size"), human_size(p.file_size)),
                (t("文件名", "File"), p.original_filename.clone()),
            ];
            for (label, value) in rows {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).size(12.0).color(theme::TEXT_WEAK));
                    ui.label(RichText::new(value).size(13.0).color(theme::TEXT));
                });
            }
            ui.separator();
            let (label, color) = match p.status {
                Status::Untreated => (t("未处理", "Untreated"), theme::TEXT_SECONDARY),
                Status::Delete => (t("待删", "Delete"), theme::DELETE),
                Status::Reviewed => (t("已阅", "Reviewed"), theme::KEEP),
            };
            ui.label(RichText::new(format!("{}: {label}", t("状态", "Status"))).size(14.0).color(color).strong());
            // Decode-state hints (PRD 3.4 / 7.4.3 / 2.3).
            if p.decode_failed {
                ui.label(
                    RichText::new(t("RAW 解码失败，当前显示内嵌预览", "RAW decode failed — showing embedded preview"))
                        .size(12.0)
                        .color(theme::ACCENT),
                );
            }
            if p.preview_only {
                ui.label(
                    RichText::new(t("此格式暂不支持完整解码", "This format can't be fully decoded"))
                        .size(12.0)
                        .color(theme::ACCENT),
                );
            }
            // Manual rotation state + reset (PRD 3.4 / 4.7).
            let turns = p.rotation_override.rem_euclid(4);
            if turns != 0 {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} {}°", t("已旋转", "Rotated"), turns * 90))
                            .size(12.0)
                            .color(theme::TEXT_SECONDARY),
                    );
                    if ui
                        .small_button(t("重置（Shift+R）", "Reset (Shift+R)"))
                        .clicked()
                    {
                        let id = p.id;
                        if let Err(e) = app.state.set_rotation_for(id, 0) {
                            app.toast(
                                ToastKind::Error,
                                format!("{}{e}", t("重置旋转失败：", "Reset rotation failed: ")),
                            );
                        } else {
                            app.toast(
                                ToastKind::Info,
                                t("已重置为 EXIF 方向", "Reset to EXIF orientation"),
                            );
                        }
                        app.needs_save = true;
                    }
                });
            }

            // Histogram (PRD 7.5) — computed lazily from the preview cache.
            ui.separator();
            draw_histogram(app, ui, p.id, p.thumb_hash.as_deref().unwrap_or(""));
        }
        None => {
            ui.label(RichText::new(t("未选择照片", "No photo selected")).color(theme::TEXT_WEAK));
        }
    }
}

/// Draw the current photo's histogram in the right panel (PRD 7.5).
fn draw_histogram(app: &mut KakaApp, ui: &mut egui::Ui, photo_id: i64, hash: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(t("直方图", "Histogram"))
                .size(12.0)
                .color(theme::TEXT_SECONDARY)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // ▾ 通道模式菜单 (PRD 7.5-1)；直方图右键打开同款。
            ui.menu_button("▼", |ui| histogram_mode_menu(app, ui))
                .response
                .on_hover_text(t("直方图通道模式", "Histogram channel mode"));
        });
    });
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 110.0),
        egui::Sense::click_and_drag(),
    );
    resp.context_menu(|ui| histogram_mode_menu(app, ui));
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, theme::PREVIEW_BG);
    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);

    if hash.is_empty() {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            t("无缓存", "No cache"),
            egui::FontId::proportional(12.0),
            theme::TEXT_WEAK,
        );
        return;
    }
    if !app.state.ensure_histogram(photo_id, hash) {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            t("预览生成后显示", "Available once the preview is generated"),
            egui::FontId::proportional(12.0),
            theme::TEXT_WEAK,
        );
        return;
    }
    let Some(h) = app.state.histogram_for(photo_id) else {
        return;
    };
    plot_histogram(
        &painter,
        rect,
        h,
        app.state.config.show_clipping_warning,
        app.state.config.histogram_mode,
    );
}

/// 通道模式菜单体 (PRD 7.5-1)：▾ 按钮与直方图右键共用，当前项打勾。
fn histogram_mode_menu(app: &mut KakaApp, ui: &mut egui::Ui) {
    use crate::model::HistogramMode;
    let current = app.state.config.histogram_mode;
    for (mode, label) in [
        (HistogramMode::Rgb, t("RGB 叠加", "RGB overlay")),
        (HistogramMode::R, t("仅R", "R only")),
        (HistogramMode::G, t("仅G", "G only")),
        (HistogramMode::B, t("仅B", "B only")),
        (HistogramMode::Luma, t("仅L", "Luma only")),
    ] {
        if ui.radio(current == mode, label).clicked() {
            app.state.config.histogram_mode = mode;
            let _ = crate::config::save(&app.state.config);
        }
    }
}

/// Overlay 4 polyline curves (R/G/B/L), plus overflow markers at the edges.
fn plot_histogram(
    painter: &egui::Painter,
    rect: egui::Rect,
    h: &crate::io::histogram::Histogram,
    show_clipping: bool,
    mode: crate::model::HistogramMode,
) {
    use crate::model::HistogramMode;
    let w = rect.width();
    let height = rect.height();
    // 按模式只画对应曲线 (PRD 7.5-1)。
    let channels: Vec<(&[u32; 256], egui::Color32)> = match mode {
        HistogramMode::Rgb => vec![
            (&h.r, egui::Color32::from_rgb(0xff, 0x55, 0x55)),
            (&h.g, egui::Color32::from_rgb(0x55, 0xe8, 0x55)),
            (&h.b, egui::Color32::from_rgb(0x55, 0x90, 0xff)),
        ],
        HistogramMode::R => vec![(&h.r, egui::Color32::from_rgb(0xff, 0x55, 0x55))],
        HistogramMode::G => vec![(&h.g, egui::Color32::from_rgb(0x55, 0xe8, 0x55))],
        HistogramMode::B => vec![(&h.b, egui::Color32::from_rgb(0x55, 0x90, 0xff))],
        HistogramMode::Luma => vec![(&h.l, egui::Color32::from_rgb(0xe0, 0xe0, 0xe0))],
    };
    for (arr, color) in channels {
        let maxv = arr.iter().copied().max().unwrap_or(1).max(1) as f32;
        let mut pts: Vec<egui::Pos2> = Vec::with_capacity(256);
        for (i, &c) in arr.iter().enumerate() {
            let x = rect.min.x + (i as f32 / 255.0) * w;
            let f = c as f32 / maxv;
            let y = rect.max.y - 3.0 - f * (height - 6.0);
            pts.push(egui::pos2(x, y));
        }
        painter.add(egui::Shape::line(pts, egui::Stroke::new(1.0, color)));
    }

    // Overflow warnings (PRD 7.5): red ticks on the clipping edges, gated by
    // the 显示高光/暗部溢出提示 setting.
    if !show_clipping {
        return;
    }
    let warn = 0.03f32;
    if h.black_ratio() > warn {
        let bx = rect.min.x + 3.0;
        painter.add(egui::Shape::line(
            vec![egui::pos2(bx, rect.min.y + 4.0), egui::pos2(bx, rect.max.y - 4.0)],
            egui::Stroke::new(2.0, theme::DELETE),
        ));
        painter.text(
            egui::pos2(rect.min.x + 6.0, rect.min.y + 1.0),
            Align2::LEFT_TOP,
            t("暗部死黑", "Shadow clipping"),
            egui::FontId::proportional(10.0),
            theme::DELETE,
        );
    }
    if h.white_ratio() > warn {
        let bx = rect.max.x - 3.0;
        painter.add(egui::Shape::line(
            vec![egui::pos2(bx, rect.min.y + 4.0), egui::pos2(bx, rect.max.y - 4.0)],
            egui::Stroke::new(2.0, theme::DELETE),
        ));
        painter.text(
            egui::pos2(rect.max.x - 6.0, rect.min.y + 1.0),
            Align2::RIGHT_TOP,
            t("高光溢出", "Highlight clipping"),
            egui::FontId::proportional(10.0),
            theme::DELETE,
        );
    }
}

fn human_size(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn render_empty_state(app: &mut KakaApp, ui: &mut egui::Ui) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.label(RichText::new("📷").size(48.0).color(theme::TEXT_WEAK));
            ui.label(RichText::new(t("暂无打开的工作区", "No workspace open")).size(20.0).strong().color(theme::TEXT));
            ui.label(RichText::new(t("开始导入你的第一批照片，或从已有文件夹开始筛选。", "Import your first photos, or pick an existing folder to start culling.")).size(14.0).color(theme::TEXT_SECONDARY));
            ui.label(RichText::new(t("所有操作仅索引，不修改原文件。", "Index-only: your source files are never modified.")).size(14.0).color(theme::TEXT_SECONDARY));
            ui.add_space(16.0);
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(t("添加硬盘文件夹", "Add disk folder"))
                            .size(15.0)
                            .strong()
                            .color(egui::Color32::from_rgb(0x12, 0x12, 0x12)),
                    )
                    .fill(theme::ACCENT)
                    .stroke(egui::Stroke::new(1.0, theme::ACCENT)),
                )
                .clicked()
            {
                app.state.show_import = true;
            }
            ui.add_space(12.0);
            ui.label(RichText::new(t("提示：你也可以直接将文件夹拖入窗口 →", "Tip: you can also drop a folder onto the window")).size(12.0).color(theme::TEXT_WEAK));
        });
    });
}
