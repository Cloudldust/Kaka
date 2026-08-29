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
                    let result = decode_full_rgba(&job.path);
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
    pub fn request(&mut self, photo_id: i64, path: &Path) {
        if self.pending.contains(&photo_id) {
            return;
        }
        self.pending.insert(photo_id);
        let _ = self.tx.send(Job {
            photo_id,
            path: path.to_path_buf(),
        });
    }

    /// True while a decode for this photo is queued or running.
    pub fn is_pending(&self, photo_id: i64) -> bool {
        self.pending.contains(&photo_id)
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

fn decode_full_rgba(path: &Path) -> Result<DecodedFull, String> {
    let img = crate::io::thumbnails::decode_full_res(path)
        .ok_or_else(|| "无法解码此文件（格式不支持或数据损坏）".to_string())?;
    let rgba = img.to_rgba8();
    Ok(DecodedFull {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}
