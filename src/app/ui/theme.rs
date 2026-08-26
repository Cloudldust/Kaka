//! Color scheme and font setup (UI Spec 1.1, 1.3, 15.1).

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily};

/// Warm orange accent.
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0xb3, 0x47);
/// Delete red-orange.
pub const DELETE: Color32 = Color32::from_rgb(0xff, 0x6b, 0x4a);
/// Keep green.
pub const KEEP: Color32 = Color32::from_rgb(0x4c, 0xdb, 0x8a);
/// Blue accent (used for histogram B channel / focus).
pub const BLUE: Color32 = Color32::from_rgb(0x6b, 0xa8, 0xff);

pub const BG: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1a);
pub const PREVIEW_BG: Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0a);
pub const TOP_BAR_BG: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
pub const RIGHT_PANEL_BG: Color32 = Color32::from_rgb(0x16, 0x16, 0x16);
pub const THUMB_STRIP_BG: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
pub const STATUS_BAR_BG: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);

pub const BORDER: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x2e);
pub const BORDER_2: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);
pub const BORDER_LIGHT: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);

pub const TEXT: Color32 = Color32::from_rgb(0xe0, 0xe0, 0xe0);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);
pub const TEXT_WEAK: Color32 = Color32::from_rgb(0x66, 0x66, 0x66);

/// Style constants (hard-corner theme, dark background).
pub const WINDOW_MIN: egui::Vec2 = egui::vec2(1024.0, 640.0);
pub const TOP_BAR_HEIGHT: f32 = 56.0;
pub const PROGRESS_HEIGHT: f32 = 4.0;
pub const STATUS_BAR_HEIGHT: f32 = 40.0;
pub const RIGHT_PANEL_DEFAULT_W: f32 = 260.0;
pub const THUMB_STRIP_DEFAULT_H: f32 = 120.0;

/// Apply the dark, hard-corner theme.
pub fn apply_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = BG;
        style.visuals.window_fill = Color32::from_rgb(0x1f, 0x1f, 0x1f);
        style.visuals.extreme_bg_color = Color32::from_rgb(0x14, 0x14, 0x14);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(0x2a, 0x2a, 0x2a);
        style.visuals.widgets.inactive.fg_stroke.width = 1.0;
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x33, 0x33, 0x33);
        style.visuals.widgets.active.bg_fill = Color32::from_rgb(0x3d, 0x3d, 0x3d);
        style.visuals.selection.bg_fill = ACCENT.linear_multiply(0.35);
        style.visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.window_margin = egui::Margin::same(16);
        style.spacing.interact_size.y = 24.0;
        style.visuals.collapsing_header_frame = true;
        // Hard corners (no rounding).
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
        style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
        style.visuals.widgets.active.corner_radius = egui::CornerRadius::ZERO;
        style.visuals.window_corner_radius = egui::CornerRadius::ZERO;
    });
}

/// Load system fonts (Segoe UI + Microsoft YaHei) so CJK renders. Falls back
/// silently to egui's bundled fonts if the OS fonts are missing.
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let wf = std::path::PathBuf::from(r"C:\Windows\Fonts");

    // Prefer Segoe UI Variable, fall back to Microsoft YaHei for CJK glyphs.
    let mut cjk_loaded = false;
    for cand in ["msyh.ttc", "msyh.ttf", "simhei.ttf"] {
        let p = wf.join(cand);
        if let Ok(bytes) = std::fs::read(&p) {
            fonts
                .font_data
                .insert("kaka_cjk".to_string(), std::sync::Arc::new(FontData::from_owned(bytes)));
            cjk_loaded = true;
            break;
        }
    }
    if !cjk_loaded {
        log::warn!("fonts: no CJK system font found (msyh/simhei); Chinese may not render");
    } else {
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let list = fonts.families.entry(family).or_default();
            // Insert CJK as the last fallback so Latin uses the default font first.
            list.push("kaka_cjk".to_string());
        }
    }

    // Segoe UI Variable / Segoe UI for crisp Latin text.
    for cand in ["segoeuivariable.ttf", "segoeui.ttf"] {
        let p = wf.join(cand);
        if let Ok(bytes) = std::fs::read(&p) {
            fonts.font_data.insert(
                "kaka_latin".to_string(),
                std::sync::Arc::new(FontData::from_owned(bytes)),
            );
            if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
                if !list.is_empty() {
                    let mut new_list = vec!["kaka_latin".to_string()];
                    new_list.append(list);
                    *list = new_list;
                }
            }
            break;
        }
    }

    ctx.set_fonts(fonts);
}
