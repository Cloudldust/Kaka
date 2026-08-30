//! Remappable keyboard shortcuts (PRD 7.2.1 / UI spec 5.3.2).
//!
//! Actions are identified by stable string codes; each action has default
//! binding codes ("Q", "Ctrl+Z", "ArrowRight", …) the user can override in
//! 设置 → 快捷键. Overrides live in config.toml (`keybindings` map) and are
//! applied live from `AppConfig` at dispatch time.
//!
//! Reserved keys that can never be bound (PRD 7.2.1): Esc, Home/End, digit
//! jump (0-9 + Enter), Ctrl+0, Ctrl+batch (Q/E/U), Ctrl+I/O import, F11,
//! and Shift/Alt modifier combos.

use eframe::egui;
use std::collections::HashMap;

/// (action code, label_zh, label_en), in settings display order.
pub const ACTIONS: &[(&str, &str, &str)] = &[
    ("mark_delete", "标记待删", "Mark for deletion"),
    ("mark_reviewed", "标记已阅跳过", "Mark reviewed / skip"),
    ("mark_untreated", "重置为未处理", "Reset to unprocessed"),
    ("next_photo", "下一张", "Next photo"),
    ("prev_photo", "上一张", "Previous photo"),
    ("toggle_zoom", "100% 放大切换", "Toggle 100% zoom"),
    ("toggle_panel", "折叠/展开右侧面板", "Toggle info panel"),
    ("select_all", "全选当前视图", "Select all"),
    ("undo", "撤销", "Undo"),
    ("redo", "重做", "Redo"),
    ("save", "保存工作区", "Save workspace"),
];

/// Ctrl+ combos reserved for batch marks and import (never bindable).
const RESERVED_CODES: &[&str] = &["Ctrl+Q", "Ctrl+E", "Ctrl+U", "Ctrl+I", "Ctrl+O"];

fn is_reserved(code: &str) -> bool {
    RESERVED_CODES.contains(&code)
}

/// Default bindings for one action. `next_photo` / `prev_photo` carry the
/// classic multi-key defaults (arrows + A/D + Space).
pub fn default_codes(action: &str) -> &'static [&'static str] {
    match action {
        "mark_delete" => &["Q"],
        "mark_reviewed" => &["E"],
        "mark_untreated" => &["U"],
        "next_photo" => &["ArrowRight", "D", "Space"],
        "prev_photo" => &["ArrowLeft", "A"],
        "toggle_zoom" => &["Z"],
        "toggle_panel" => &["I"],
        "select_all" => &["Ctrl+A"],
        "undo" => &["Ctrl+Z"],
        "redo" => &["Ctrl+Y"],
        "save" => &["Ctrl+S"],
        _ => &[],
    }
}

/// Bindings currently in effect: the user override if set, else the defaults.
pub fn effective_codes(map: &HashMap<String, String>, action: &str) -> Vec<String> {
    match map.get(action) {
        Some(code) => vec![code.clone()],
        None => default_codes(action).iter().map(|s| s.to_string()).collect(),
    }
}

/// Convert a pressed key into a binding code. Returns None for keys that are
/// not bindable at all (Esc/Home/End/digits/F-keys) or unsupported modifier
/// combos — only plain keys and Ctrl+key are allowed (PRD 7.2.1).
pub fn encode(mods: egui::Modifiers, key: egui::Key) -> Option<String> {
    let name = key_name(key)?;
    if mods.ctrl && !mods.shift && !mods.alt {
        Some(format!("Ctrl+{name}"))
    } else if !mods.ctrl && !mods.shift && !mods.alt {
        Some(name.to_string())
    } else {
        None
    }
}

fn key_name(key: egui::Key) -> Option<&'static str> {
    let name = match key {
        egui::Key::A => "A",
        egui::Key::B => "B",
        egui::Key::C => "C",
        egui::Key::D => "D",
        egui::Key::E => "E",
        egui::Key::F => "F",
        egui::Key::G => "G",
        egui::Key::H => "H",
        egui::Key::I => "I",
        egui::Key::J => "J",
        egui::Key::K => "K",
        egui::Key::L => "L",
        egui::Key::M => "M",
        egui::Key::N => "N",
        egui::Key::O => "O",
        egui::Key::P => "P",
        egui::Key::Q => "Q",
        egui::Key::R => "R",
        egui::Key::S => "S",
        egui::Key::T => "T",
        egui::Key::U => "U",
        egui::Key::V => "V",
        egui::Key::W => "W",
        egui::Key::X => "X",
        egui::Key::Y => "Y",
        egui::Key::Z => "Z",
        egui::Key::Space => "Space",
        egui::Key::ArrowLeft => "ArrowLeft",
        egui::Key::ArrowRight => "ArrowRight",
        egui::Key::ArrowUp => "ArrowUp",
        egui::Key::ArrowDown => "ArrowDown",
        _ => return None,
    };
    Some(name)
}

/// Parse a stored code back into (ctrl, key) for dispatch.
fn parse(code: &str) -> Option<(bool, egui::Key)> {
    let (ctrl, name) = match code.strip_prefix("Ctrl+") {
        Some(rest) => (true, rest),
        None => (false, code),
    };
    let key = match name {
        "Space" => egui::Key::Space,
        "ArrowLeft" => egui::Key::ArrowLeft,
        "ArrowRight" => egui::Key::ArrowRight,
        "ArrowUp" => egui::Key::ArrowUp,
        "ArrowDown" => egui::Key::ArrowDown,
        single
            if single.len() == 1 && single.as_bytes()[0].is_ascii_alphabetic() =>
        {
            let c = single.chars().next()?.to_ascii_uppercase();
            match c {
            'A' => egui::Key::A,
            'B' => egui::Key::B,
            'C' => egui::Key::C,
            'D' => egui::Key::D,
            'E' => egui::Key::E,
            'F' => egui::Key::F,
            'G' => egui::Key::G,
            'H' => egui::Key::H,
            'I' => egui::Key::I,
            'J' => egui::Key::J,
            'K' => egui::Key::K,
            'L' => egui::Key::L,
            'M' => egui::Key::M,
            'N' => egui::Key::N,
            'O' => egui::Key::O,
            'P' => egui::Key::P,
            'Q' => egui::Key::Q,
            'R' => egui::Key::R,
            'S' => egui::Key::S,
            'T' => egui::Key::T,
            'U' => egui::Key::U,
            'V' => egui::Key::V,
            'W' => egui::Key::W,
            'X' => egui::Key::X,
            'Y' => egui::Key::Y,
            'Z' => egui::Key::Z,
            _ => return None,
            }
        }
        _ => return None,
    };
    Some((ctrl, key))
}

/// Consume the key described by `code` if it was pressed this frame.
pub fn consume(ctx: &egui::Context, code: &str) -> bool {
    let Some((ctrl, key)) = parse(code) else {
        return false;
    };
    let mods = if ctrl { egui::Modifiers::CTRL } else { egui::Modifiers::NONE };
    ctx.input_mut(|i| i.consume_key(mods, key))
}

/// Human-readable binding for buttons and the status-bar hint.
pub fn display(code: &str) -> String {
    match code {
        "ArrowLeft" => "←".into(),
        "ArrowRight" => "→".into(),
        other => other.to_string(),
    }
}

/// Validate a new binding for `action`: reserved keys and conflicts with other
/// actions are rejected (PRD 7.2.1 冲突处理). Returns () or a user-facing
/// error message in the current language.
pub fn validate(map: &HashMap<String, String>, action: &str, code: &str) -> Result<(), String> {
    if is_reserved(code) {
        return Err(crate::i18n::t(
            "该键为系统保留键（Ctrl+批量/导入），无法绑定",
            "Reserved key (Ctrl+batch/import) cannot be bound",
        )
        .to_string());
    }
    for (other, zh, en) in ACTIONS {
        if *other == action {
            continue;
        }
        if effective_codes(map, other).iter().any(|c| c == code) {
            let label = crate::i18n::t(zh, en);
            return Err(match crate::i18n::lang() {
                crate::i18n::Lang::Zh => format!("与「{label}」冲突，请先更换该功能的按键"),
                crate::i18n::Lang::En => format!("Conflicts with '{label}' — rebind that action first"),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_plain_ctrl_and_rejects() {
        assert_eq!(encode(egui::Modifiers::NONE, egui::Key::Q).as_deref(), Some("Q"));
        assert_eq!(encode(egui::Modifiers::CTRL, egui::Key::Z).as_deref(), Some("Ctrl+Z"));
        // Shift/Alt combos are not bindable.
        let mut m = egui::Modifiers::CTRL;
        m.shift = true;
        assert_eq!(encode(m, egui::Key::Z), None);
        // Reserved / unbindable keys have no name.
        assert_eq!(encode(egui::Modifiers::NONE, egui::Key::Escape), None);
        assert_eq!(encode(egui::Modifiers::NONE, egui::Key::Num5), None);
        assert_eq!(encode(egui::Modifiers::NONE, egui::Key::Home), None);
        assert_eq!(encode(egui::Modifiers::NONE, egui::Key::F11), None);
    }

    #[test]
    fn effective_defaults_then_override() {
        let map = HashMap::new();
        assert_eq!(
            effective_codes(&map, "next_photo"),
            vec!["ArrowRight".to_string(), "D".to_string(), "Space".to_string()]
        );
        let mut map = HashMap::new();
        map.insert("next_photo".to_string(), "J".to_string());
        assert_eq!(effective_codes(&map, "next_photo"), vec!["J".to_string()]);
        assert_eq!(effective_codes(&map, "mark_delete"), vec!["Q".to_string()]);
    }

    #[test]
    fn validate_rejects_reserved_and_conflicts() {
        let empty = HashMap::new();
        // Conflicts with the default binding of another action.
        assert!(validate(&empty, "next_photo", "Q").is_err());
        assert!(validate(&empty, "mark_reviewed", "Space").is_err());
        // Rebinding an action to its own current key is fine.
        assert!(validate(&empty, "mark_delete", "Q").is_ok());
        // Reserved Ctrl+batch/import keys rejected.
        assert!(validate(&empty, "mark_delete", "Ctrl+Q").is_err());
        assert!(validate(&empty, "mark_delete", "Ctrl+O").is_err());
        // Once mark_delete moved away, Q is free for others.
        let mut moved = HashMap::new();
        moved.insert("mark_delete".to_string(), "R".to_string());
        assert!(validate(&moved, "next_photo", "Q").is_ok());
    }

    #[test]
    fn parse_roundtrip() {
        for code in ["Q", "M", "Ctrl+Z", "Ctrl+A", "Space", "ArrowRight", "ArrowDown"] {
            let parsed = parse(code).map(|(ctrl, key)| {
                encode(if ctrl { egui::Modifiers::CTRL } else { egui::Modifiers::NONE }, key)
            });
            assert_eq!(parsed, Some(Some(code.to_string())), "roundtrip failed for {code}");
        }
        assert!(parse("Ctrl+5").is_none());
        assert!(parse("Home").is_none());
    }
}
