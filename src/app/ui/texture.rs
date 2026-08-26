//! UI-side thumbnail/preview texture cache. Photos are keyed by (id, thumb_hash).
//!
//! Loading never blocks the UI thread: `texture_for` only reads an already-cached
//! file; when a cache is missing it returns a placeholder and reports `needs_gen`,
//! and the caller enqueues a background generation job (see `app::thumbs`).

use crate::model::PhotoListItem;
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct TextureCache {
    thumb_map: HashMap<(i64, String), TextureHandle>,
    preview_map: HashMap<(i64, String), TextureHandle>,
    placeholder: Option<TextureHandle>,
}

impl Default for TextureCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TextureCache {
    pub fn new() -> Self {
        TextureCache {
            thumb_map: HashMap::new(),
            preview_map: HashMap::new(),
            placeholder: None,
        }
    }

    fn placeholder(ctx: &egui::Context) -> TextureHandle {
        let size = 4;
        let mut data = vec![0u8; size * size * 4];
        for px in data.chunks_exact_mut(4) {
            px[0] = 0x2a;
            px[1] = 0x2a;
            px[2] = 0x2a;
            px[3] = 0xff;
        }
        let img = ColorImage::from_rgba_unmultiplied([size, size], &data);
        ctx.load_texture("kaka-placeholder", img, TextureOptions::NEAREST)
    }

    pub fn placeholder_handle(&mut self, ctx: &egui::Context) -> TextureHandle {
        if let Some(t) = &self.placeholder {
            return t.clone();
        }
        let t = Self::placeholder(ctx);
        self.placeholder = Some(t.clone());
        t
    }

    /// Get the thumbnail texture for a photo. Returns (texture, needs_gen).
    /// `needs_gen` is true when the cache file is missing and the caller should
    /// enqueue a background generation job.
    pub fn texture_for(&mut self, ctx: &egui::Context, photo: &PhotoListItem) -> (TextureHandle, bool) {
        let hash = photo.thumb_hash.clone().unwrap_or_default();
        if hash.is_empty() {
            return (self.placeholder_handle(ctx), false);
        }
        let key = (photo.id, hash.clone());
        if let Some(tex) = self.thumb_map.get(&key) {
            return (tex.clone(), false);
        }
        if let Some(tex) = load_thumbnail(ctx, photo, &hash) {
            self.thumb_map.insert(key, tex.clone());
            (tex, false)
        } else {
            (self.placeholder_handle(ctx), true)
        }
    }

    /// Get the large-preview texture, falling back to the thumbnail.
    /// Returns (texture, needs_gen).
    pub fn preview_for(&mut self, ctx: &egui::Context, photo: &PhotoListItem) -> (TextureHandle, bool) {
        let hash = photo.thumb_hash.clone().unwrap_or_default();
        if hash.is_empty() {
            return (self.placeholder_handle(ctx), false);
        }
        let key = (photo.id, hash.clone());
        if let Some(tex) = self.preview_map.get(&key) {
            return (tex.clone(), false);
        }
        if let Some(tex) = load_preview(ctx, photo, &hash) {
            self.preview_map.insert(key, tex.clone());
            (tex, false)
        } else {
            // No preview cache yet: fall back to the thumbnail texture, and
            // signal that a generation job is still needed for the preview.
            let (t, n) = self.texture_for(ctx, photo);
            (t, n)
        }
    }

    /// Drop cached entries for a photo/hash so they reload after regeneration.
    pub fn invalidate(&mut self, photo_id: i64, hash: &str) {
        self.thumb_map.remove(&(photo_id, hash.to_string()));
        self.preview_map.remove(&(photo_id, hash.to_string()));
    }

    /// Drop cached entries for photos no longer in the current workspace.
    pub fn retain(&mut self, valid_ids: &std::collections::HashSet<i64>) {
        self.thumb_map.retain(|(id, _), _| valid_ids.contains(id));
        self.preview_map.retain(|(id, _), _| valid_ids.contains(id));
    }
}

/// Read a thumbnail from the disk cache. Returns None when the file is missing
/// (caller should enqueue generation) — never generates on the UI thread.
fn load_thumbnail(ctx: &egui::Context, photo: &PhotoListItem, hash: &str) -> Option<TextureHandle> {
    let path = crate::io::thumbnails::thumb_path(hash, 1.0);
    if !path.exists() {
        return None;
    }
    decode_to_texture(ctx, &path, format!("thumb-{}-{}", photo.id, hash))
}

fn load_preview(ctx: &egui::Context, photo: &PhotoListItem, hash: &str) -> Option<TextureHandle> {
    let path = crate::io::thumbnails::preview_path(hash);
    if !path.exists() {
        return None;
    }
    decode_to_texture(ctx, &path, format!("preview-{}-{}", photo.id, hash))
}

fn decode_to_texture(ctx: &egui::Context, path: &Path, tag: String) -> Option<TextureHandle> {
    let bytes = std::fs::read(path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let color = ColorImage::from_rgba_unmultiplied([w, h], rgba.as_raw());
    Some(ctx.load_texture(tag, color, TextureOptions::LINEAR))
}

/// Decode an image straight from a source file (best-effort).
#[allow(dead_code)]
pub fn load_preview_texture_from_source(ctx: &egui::Context, path: &Path, tag: &str) -> Option<TextureHandle> {
    decode_to_texture(ctx, path, tag.to_string())
}

/// Avoid unused-import warnings.
#[allow(dead_code)]
fn _unused(_p: PathBuf) {}
