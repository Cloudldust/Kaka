//! Main window layout (UI Spec 2, 3): top bar, progress, preview, right panel,
//! thumbnail strip, status bar, empty state.

use super::app::KakaApp;
use super::theme;
use crate::model::{PhotoListItem, SortOrder, Status};
use eframe::egui::{self, Align, Align2, Layout, RichText};

pub fn render(app: &mut KakaApp, ui: &mut egui::Ui) {
    render_top_bottom_panels(app, ui);
}

fn render_top_bottom_panels(app: &mut KakaApp, ui: &mut egui::Ui) {
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
                let label = truncate_path(&path, 44);
                ui.label(RichText::new(label).size(14.0).color(theme::TEXT))
                    .on_hover_text(if path.is_empty() {
                        "未打开文件夹".to_string()
                    } else {
                        path.clone()
                    });

                ui.separator();
                sort_dropdown(app, ui);

                ui.add_space(8.0);
                ui.label(RichText::new("🔍").size(14.0).color(theme::TEXT_SECONDARY));
                let mut search = app.state.ws.search.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut search)
                        .desired_width(200.0)
                        .hint_text("搜索文件名 / @过滤条件"),
                );
                if resp.changed() {
                    app.state.ws.search = search;
                    let s = app.state.ws.search.clone();
                    apply_search(app, &s);
                }
                if !app.state.ws.search.is_empty() && ui.button("✕").clicked() {
                    app.state.ws.search.clear();
                    apply_search(app, "");
                }

                // Advanced filter (PRD 7.8) button.
                let filter_active = app.state.ws.filter.is_active();
                let filter_btn_style = if filter_active {
                    egui::Button::new(
                        RichText::new("过滤").strong().color(egui::Color32::from_rgb(0x12, 0x12, 0x12)),
                    )
                    .fill(theme::ACCENT)
                    .stroke(egui::Stroke::new(1.0, theme::ACCENT))
                } else {
                    egui::Button::new(RichText::new("过滤").color(theme::TEXT_SECONDARY))
                };
                if ui.add(filter_btn_style).clicked() {
                    app.filter_draft = app.state.ws.filter.clone();
                    app.state.show_filter = true;
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let del = app.state.ws.counts.deleted;
                    let del_text = format!("待删 ({del})");
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
                        RichText::new("导入")
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
                    let export_btn = egui::Button::new(RichText::new("导出").size(15.0).color(theme::TEXT_SECONDARY));
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
                        app.lr_path = crate::app::export::lr_install_path();
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
                    ui.label(RichText::new("暂无照片").color(theme::TEXT_WEAK));
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
        .selected_text(format!("☰ 排序: {}", current.label()))
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
        if ui.add(gear).on_hover_text("设置").clicked() {
            app.open_settings();
        }
        ui.separator();

        let counts = &app.state.ws.counts;
        let processed = counts.deleted + counts.reviewed;
        let total = counts.total;
        let green = total > 0 && processed >= total;
        let frac_color = if green { theme::KEEP } else { theme::TEXT };
        ui.label(
            RichText::new(format!("已筛选 {processed} / {total}"))
                .size(14.0)
                .color(frac_color)
                .strong(),
        );
        sep(ui);
        ui.label(RichText::new(format!("保留 {}", total - counts.deleted)).size(14.0).color(theme::KEEP));
        sep(ui);
        ui.label(RichText::new(format!("已阅 {}", counts.reviewed)).size(14.0).color(theme::TEXT_SECONDARY));
        sep(ui);
        ui.label(RichText::new(format!("待删 {}", counts.deleted)).size(14.0).color(theme::DELETE));
        if app.state.ws.selected_count() > 0 {
            sep(ui);
            ui.label(RichText::new(format!("选中 {}", app.state.ws.selected_count())).size(14.0).color(theme::ACCENT));
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new("Q 待删 | E 跳过 | ← → 切换").size(13.0).color(theme::TEXT_WEAK));
        });
    });
}

fn sep(ui: &mut egui::Ui) {
    ui.label(RichText::new("|").color(theme::BORDER_2));
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
                    let (clicked_item, rect) = thumb_widget(ui, &tex, item, is_current, is_selected);
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
        let scale = (img_rect.width() / ts.x).min(img_rect.height() / ts.y);
        let size = ts * scale;
        let draw_rect = egui::Rect::from_center_size(img_rect.center(), size);
        painter.image(
            tex.id(),
            draw_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
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
                "待删",
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
                "已阅",
                egui::FontId::proportional(10.0),
                egui::Color32::from_rgb(0x0f, 0x2a, 0x1c),
            );
        }
        Status::Untreated => {}
    }
    if item.pair_group_id.is_some() && item.status != Status::Delete {
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
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, theme::PREVIEW_BG);

    if let Some(item) = app.state.ws.current().cloned() {
        // Reset zoom when the current photo changes (anchor is per photo, PRD 7.4).
        if app.zoom_photo_id != Some(item.id) {
            app.zoom_active = false;
            app.zoom_offset = egui::Vec2::ZERO;
            app.zoom_photo_id = Some(item.id);
        }

        let (tex, needs) = app.textures.preview_for(ui.ctx(), &item);
        if needs {
            let hash = item.thumb_hash.clone().unwrap_or_default();
            app.thumbs.enqueue(item.id, &hash, &item.current_path);
        }
        let ts = tex.size_vec2();
        if ts.x > 0.0 && ts.y > 0.0 {
            let draw_rect;
            if app.zoom_active {
                // 100%: 1 image px = 1 screen px; pan with Ctrl+drag (PRD 7.4).
                let pan = ui.input(|i| i.modifiers.ctrl);
                if pan && resp.dragged() {
                    app.zoom_offset += resp.drag_delta();
                }
                let mut top_left = rect.center() - ts * 0.5 + app.zoom_offset;
                // Clamp so the image always covers the view when it is larger
                // than the window; otherwise keep it centered.
                if ts.x >= rect.width() {
                    top_left.x = top_left.x.clamp(rect.max.x - ts.x, rect.min.x);
                } else {
                    top_left.x = rect.center().x - ts.x * 0.5;
                }
                if ts.y >= rect.height() {
                    top_left.y = top_left.y.clamp(rect.max.y - ts.y, rect.min.y);
                } else {
                    top_left.y = rect.center().y - ts.y * 0.5;
                }
                draw_rect = egui::Rect::from_min_size(top_left, ts);
            } else {
                let margin = 24.0;
                let avail = egui::vec2(rect.width() - margin * 2.0, rect.height() - margin * 2.0)
                    .max(egui::vec2(1.0, 1.0));
                let scale = (avail.x / ts.x).min(avail.y / ts.y);
                let size = ts * scale;
                draw_rect = egui::Rect::from_center_size(rect.center(), size);
            }
            painter.image(
                tex.id(),
                draw_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );

            if app.zoom_active {
                painter.text(
                    egui::pos2(draw_rect.min.x + 6.0, draw_rect.min.y + 6.0),
                    Align2::LEFT_TOP,
                    "100%  Ctrl+拖拽平移",
                    egui::FontId::proportional(12.0),
                    theme::ACCENT,
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
                        "待删",
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
                        "已阅",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_rgb(0x0f, 0x2a, 0x1c),
                    );
                }
                Status::Untreated => {}
            }
        }
    }
}

fn draw_right_panel(app: &mut KakaApp, ui: &mut egui::Ui) {
    ui.add_space(8.0);
    ui.label(RichText::new("信息面板").size(12.0).color(theme::TEXT_SECONDARY).strong());
    ui.separator();
    match app.state.ws.current().cloned() {
        Some(p) => {
            let rows: Vec<(&str, String)> = vec![
                ("相机", p.camera_model.clone().unwrap_or_else(|| "—".into())),
                ("镜头", p.lens_model.clone().unwrap_or_else(|| "—".into())),
                ("焦距", p.focal_length.map(|v| format!("{v} mm")).unwrap_or_else(|| "—".into())),
                ("光圈", p.aperture.clone().unwrap_or_else(|| "—".into())),
                ("快门", p.shutter_speed.clone().unwrap_or_else(|| "—".into())),
                ("ISO", p.iso.map(|v| v.to_string()).unwrap_or_else(|| "—".into())),
                ("时间", p.capture_time.clone()),
                ("大小", human_size(p.file_size)),
                ("文件名", p.original_filename.clone()),
            ];
            for (label, value) in rows {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).size(12.0).color(theme::TEXT_WEAK));
                    ui.label(RichText::new(value).size(13.0).color(theme::TEXT));
                });
            }
            ui.separator();
            let (label, color) = match p.status {
                Status::Untreated => ("未处理", theme::TEXT_SECONDARY),
                Status::Delete => ("待删", theme::DELETE),
                Status::Reviewed => ("已阅", theme::KEEP),
            };
            ui.label(RichText::new(format!("状态: {label}")).size(14.0).color(color).strong());

            // Histogram (PRD 7.5) — computed lazily from the preview cache.
            ui.separator();
            draw_histogram(app, ui, p.id, p.thumb_hash.as_deref().unwrap_or(""));
        }
        None => {
            ui.label(RichText::new("未选择照片").color(theme::TEXT_WEAK));
        }
    }
}

/// Draw the current photo's histogram in the right panel (PRD 7.5).
fn draw_histogram(app: &mut KakaApp, ui: &mut egui::Ui, photo_id: i64, hash: &str) {
    ui.label(RichText::new("直方图").size(12.0).color(theme::TEXT_SECONDARY).strong());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 110.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, theme::PREVIEW_BG);
    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);

    if hash.is_empty() {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "无缓存",
            egui::FontId::proportional(12.0),
            theme::TEXT_WEAK,
        );
        return;
    }
    if !app.state.ensure_histogram(photo_id, hash) {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "预览生成后显示",
            egui::FontId::proportional(12.0),
            theme::TEXT_WEAK,
        );
        return;
    }
    let Some(h) = app.state.histogram_for(photo_id) else {
        return;
    };
    plot_histogram(&painter, rect, h);
}

/// Overlay 4 polyline curves (R/G/B/L), plus overflow markers at the edges.
fn plot_histogram(painter: &egui::Painter, rect: egui::Rect, h: &crate::io::histogram::Histogram) {
    let w = rect.width();
    let height = rect.height();
    let channels: [([u32; 256], egui::Color32); 4] = [
        (h.r, egui::Color32::from_rgb(0xff, 0x55, 0x55)),
        (h.g, egui::Color32::from_rgb(0x55, 0xe8, 0x55)),
        (h.b, egui::Color32::from_rgb(0x55, 0x90, 0xff)),
        (h.l, egui::Color32::from_rgb(0xe0, 0xe0, 0xe0)),
    ];
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

    // Overflow warnings (PRD 7.5): red ticks on the clipping edges.
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
            "暗部死黑",
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
            "高光溢出",
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
            ui.label(RichText::new("暂无打开的工作区").size(20.0).strong().color(theme::TEXT));
            ui.label(RichText::new("开始导入你的第一批照片，或从已有文件夹开始筛选。").size(14.0).color(theme::TEXT_SECONDARY));
            ui.label(RichText::new("所有操作仅索引，不修改原文件。").size(14.0).color(theme::TEXT_SECONDARY));
            ui.add_space(16.0);
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("添加硬盘文件夹")
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
            ui.label(RichText::new("提示：你也可以直接将文件夹拖入窗口 →").size(12.0).color(theme::TEXT_WEAK));
        });
    });
}
