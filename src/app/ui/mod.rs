//! egui/eframe UI layer (M1 skeleton).

pub mod app;
pub mod dialogs;
pub mod texture;
pub mod theme;
pub mod view;

/// Convenience re-export so callers can launch the app in one call.
pub use app::run;
