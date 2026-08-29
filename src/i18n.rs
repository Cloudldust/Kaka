//! Minimal i18n: inline Chinese/English string pairs selected by a global
//! language flag. Translations live at the call site as `t("中文", "English")`
//! — no key registry, greppable, zero deps.
//!
//! The active language comes from config.toml (`language = "zh"|"en"`),
//! applied at startup and whenever settings are saved.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};

static LANG: AtomicU8 = AtomicU8::new(0); // 0 = Zh, 1 = En

/// UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Lang {
    #[default]
    Zh,
    En,
}

impl Lang {
    pub fn from_code(code: &str) -> Self {
        if code.eq_ignore_ascii_case("en") {
            Lang::En
        } else {
            Lang::Zh
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }

    /// Shown in its own language inside the picker (中文 / English).
    pub fn native_label(self) -> &'static str {
        match self {
            Lang::Zh => "中文",
            Lang::En => "English",
        }
    }
}

pub fn set_lang(lang: Lang) {
    LANG.store(lang as u8, Ordering::SeqCst);
}

pub fn lang() -> Lang {
    if LANG.load(Ordering::SeqCst) == 1 {
        Lang::En
    } else {
        Lang::Zh
    }
}

/// Translate a constant UI string: returns the Chinese or English variant
/// depending on the active language.
pub fn t(zh: &'static str, en: &'static str) -> &'static str {
    match lang() {
        Lang::Zh => zh,
        Lang::En => en,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_code_roundtrip_and_t() {
        assert_eq!(Lang::from_code("en"), Lang::En);
        assert_eq!(Lang::from_code("EN"), Lang::En);
        assert_eq!(Lang::from_code("zh"), Lang::Zh);
        assert_eq!(Lang::from_code("garbage"), Lang::Zh);
        assert_eq!(Lang::En.code(), "en");

        set_lang(Lang::En);
        assert_eq!(t("导入", "Import"), "Import");
        set_lang(Lang::Zh);
        assert_eq!(t("导入", "Import"), "导入");
    }
}
