//! Background full-resolution decoder for the Z-key 100% view (PRD 7.4).
//!
//! When Z enters 100% mode the UI instantly shows the cached embedded-JPG
//! preview (0 延迟); a dedicated worker thread decodes the RAW at full
//! resolution (rawler develop pipeline: rescale → demosaic → white balance →
//! sRGB). The result is uploaded to a texture on the UI thread and cached in
//! a 2 GB memory LRU, after which the 100% view swaps seamlessly to true RAW
//! pixels. Decode failures persist the `decode_failed` flag (PRD 7.4.3) so
//! broken files are never retried on the next startup.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

/// A full-resolution decoded image in RGBA8.
pub struct DecodedFull {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Messages from the decode worker back to the UI.
pub enum ZoomMsg {
    /// Cheap full-resolution dimensions, sent before the heavy decode so the
    /// UI can frame the 100% view correctly while still showing the preview
    /// (the swap then keeps the exact framing — no visible jump).
    Dims {
        photo_id: i64,
        width: u32,
        height: u32,
    },
    /// Full decode outcome.
    Done {
        photo_id: i64,
        result: Result<DecodedFull, String>,
    },
}

struct Job {
    photo_id: i64,
    path: PathBuf,
    /// Thumbnail hash, used to find the disk preview as the camera-rendered
    /// tone reference (empty when unknown).
    hash: String,
}

/// Single-worker decode pool. One thread is deliberate: a RAW develop is
/// CPU-heavy (hundreds of ms to seconds), and parallel jobs would only contend.
pub struct ZoomWorker {
    tx: Sender<Job>,
    rx: Receiver<ZoomMsg>,
    pending: HashSet<i64>,
    cancel: std::sync::Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Default for ZoomWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl ZoomWorker {
    pub fn new() -> Self {
        let (job_tx, job_rx) = channel::<Job>();
        let (msg_tx, msg_rx) = channel::<ZoomMsg>();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let cancel_t = std::sync::Arc::clone(&cancel);
        let thread = std::thread::spawn(move || {
            Self::run_loop(job_rx, msg_tx, cancel_t);
        });
        ZoomWorker {
            tx: job_tx,
            rx: msg_rx,
            pending: HashSet::new(),
            cancel,
            thread: Some(thread),
        }
    }

    fn run_loop(rx: Receiver<Job>, tx: Sender<ZoomMsg>, cancel: std::sync::Arc<AtomicBool>) {
        loop {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(job) => {
                    // Cheap dimension hint first (EXIF PixelX/YDimension) so
                    // the UI frames the 100% view before the decode lands.
                    if let Some((w, h)) = crate::io::exif::pixel_dims(&job.path) {
                        let _ = tx.send(ZoomMsg::Dims {
                            photo_id: job.photo_id,
                            width: w,
                            height: h,
                        });
                    }
                    let result = decode_full_rgba(&job.path, &job.hash);
                    let _ = tx.send(ZoomMsg::Done {
                        photo_id: job.photo_id,
                        result,
                    });
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    /// Queue a full-resolution decode. Duplicate requests for a photo still in
    /// flight are ignored (`is_pending`).
    pub fn request(&mut self, photo_id: i64, path: &Path, hash: &str) {
        if self.pending.contains(&photo_id) {
            return;
        }
        self.pending.insert(photo_id);
        let _ = self.tx.send(Job {
            photo_id,
            path: path.to_path_buf(),
            hash: hash.to_string(),
        });
    }

    /// True while a decode for this photo is queued or running.
    pub fn is_pending(&self, photo_id: i64) -> bool {
        self.pending.contains(&photo_id)
    }

    /// True while ANY full-resolution decode is queued or running (PRD 9.5:
    /// background preview preloading pauses so RAW decoding gets the IO).
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Non-blocking drain of finished messages.
    pub fn poll(&mut self) -> Vec<ZoomMsg> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            if let ZoomMsg::Done { photo_id, .. } = &msg {
                self.pending.remove(photo_id);
            }
            out.push(msg);
        }
        out
    }
}

impl Drop for ZoomWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist_from(vals: &[u8]) -> [f64; 256] {
        let mut h = [0f64; 256];
        for &v in vals {
            h[v as usize] += 1.0;
        }
        h
    }

    #[test]
    fn tone_lut_is_identity_for_equal_histograms() {
        let vals: Vec<u8> = (0..=255).collect();
        let h = hist_from(&vals);
        let lut = tone_match_lut(&h, &h).unwrap();
        for i in 0..256 {
            assert!((lut[i] as i32 - i as i32).abs() <= 1, "lut[{i}]={}", lut[i]);
        }
    }

    #[test]
    fn tone_lut_lifts_when_reference_is_brighter() {
        // Raw concentrated in the shadows, reference in the midtones.
        let raw = hist_from(&[10, 11, 12, 13, 14, 15, 16, 17]);
        let cam = hist_from(&[110, 111, 112, 113, 114, 115, 116, 117]);
        let lut = tone_match_lut(&raw, &cam).unwrap();
        assert!(lut[12] > 90, "shadow 12 should map to ~midtones, got {}", lut[12]);
        // Monotonic by construction — verify explicitly.
        for i in 1..256 {
            assert!(lut[i] >= lut[i - 1]);
        }
    }

    #[test]
    fn tone_lut_rejects_degenerate_histograms() {
        assert!(tone_match_lut(&[0.0; 256], &hist_from(&[5])).is_none());
        assert!(tone_match_lut(&hist_from(&[5]), &[0.0; 256]).is_none());
    }

    #[test]
    fn channel_hist_bins_256() {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([200, 150, 100])));
        let h = channel_hist256(&img);
        let total: f64 = h[0].iter().sum();
        assert!(total > 0.0);
        // The flat image lands entirely in each channel's own bin (resize
        // upscale preserves the single color).
        assert!(h[0][200] >= total * 0.99, "R bin200={} total={total}", h[0][200]);
        assert!(h[1][150] >= total * 0.99, "G bin150={} total={total}", h[1][150]);
        assert!(h[2][100] >= total * 0.99, "B bin100={} total={total}", h[2][100]);
    }
}

/// GPU textures have a hard dimension limit (16384 on all common desktop
/// GPUs). Real camera sensors stay far below it, but a stitched RAW panorama
/// could exceed it — downscale instead of crashing the driver.
const ZOOM_MAX_TEXTURE_EDGE: u32 = 16384;

fn decode_full_rgba(path: &Path, hash: &str) -> Result<DecodedFull, String> {
    let img = crate::io::thumbnails::decode_full_res(path)
        .ok_or_else(|| "无法解码此文件（格式不支持或数据损坏）".to_string())?;
    let (w, h) = (img.width(), img.height());
    let img = if w.max(h) > ZOOM_MAX_TEXTURE_EDGE {
        let scale = ZOOM_MAX_TEXTURE_EDGE as f64 / w.max(h) as f64;
        let nw = ((w as f64) * scale).round().max(1.0) as u32;
        let nh = ((h as f64) * scale).round().max(1.0) as u32;
        img.resize(nw, nh, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    // Tone match to the camera rendering so the swap from the embedded
    // preview is seamless — the rawler develop only applies sRGB gamma and
    // reads distinctly darker than the camera/preview look.
    let img = match_camera_tone(img, path, hash);
    let rgba = img.to_rgba8();
    Ok(DecodedFull {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// Match the developed RAW's tone AND color to the camera-rendered reference
/// (disk preview or embedded preview) with per-channel 256-entry CDF
/// histogram-matching LUTs. Returns the image unchanged when no reference is
/// available. This is what keeps the preview → RAW pixels swap visually
/// seamless: the user was looking at the camera rendering, so the decode
/// should land in that same tone/hue — a single luma curve fixes brightness
/// but leaves the per-tonal-range color (greens etc.) shifted, because the
/// rawler color pipeline lacks the camera's rendering.
fn match_camera_tone(
    img: image::DynamicImage,
    path: &Path,
    hash: &str,
) -> image::DynamicImage {
    let Some(reference) = crate::io::thumbnails::camera_tone_reference(path, hash) else {
        return img;
    };
    let raw_hist = channel_hist256(&img);
    let cam_hist = channel_hist256(&reference);
    let mut rgb = img.to_rgb8();
    for (c, hist_pair) in raw_hist.iter().zip(cam_hist.iter()).enumerate() {
        let Some(lut) = tone_match_lut(hist_pair.0, hist_pair.1) else {
            continue;
        };
        for p in rgb.pixels_mut() {
            p[c] = lut[p[c] as usize];
        }
    }
    image::DynamicImage::ImageRgb8(rgb)
}

/// Per-channel 256-bin histograms of a downscaled copy (aspect-preserving).
fn channel_hist256(img: &image::DynamicImage) -> [[f64; 256]; 3] {
    let small = img
        .resize(128, 128, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let mut hist = [[0f64; 256]; 3];
    for p in small.pixels() {
        for (c, bin) in hist.iter_mut().enumerate() {
            bin[p[c] as usize] += 1.0;
        }
    }
    hist
}

/// Build a monotonic LUT mapping raw luma bins to the reference luma value at
/// the same CDF position (classic histogram matching). None when either
/// histogram is degenerate.
fn tone_match_lut(raw: &[f64; 256], cam: &[f64; 256]) -> Option<[u8; 256]> {
    let total_raw: f64 = raw.iter().sum();
    let total_cam: f64 = cam.iter().sum();
    if total_raw <= 0.0 || total_cam <= 0.0 {
        return None;
    }
    let cdf = |h: &[f64; 256], total: f64| -> [f64; 256] {
        let mut cdf = [0f64; 256];
        let mut acc = 0.0;
        for (i, v) in h.iter().enumerate() {
            acc += v;
            cdf[i] = acc / total;
        }
        cdf
    };
    let rc = cdf(raw, total_raw);
    let cc = cdf(cam, total_cam);
    let mut lut = [0u8; 256];
    let mut j = 0usize;
    for (i, lut_val) in lut.iter_mut().enumerate() {
        while j < 255 && cc[j] < rc[i] {
            j += 1;
        }
        *lut_val = j as u8;
    }
    Some(lut)
}
