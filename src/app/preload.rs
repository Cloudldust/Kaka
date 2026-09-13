//! Background pre-decode of neighbouring photos' preview textures
//! (PRD 9.5 后台预解码). While the user browses sequentially, the preview
//! textures (1920px disk-cache JPEGs) of the current photo's ±1 neighbours
//! are decoded off the UI thread and delivered as `egui::ColorImage`s; the
//! UI thread uploads them straight into the 1 GB preview MemLru, so the
//! first switch to a neighbour hits the memory cache with no disk wait.
//!
//! Idle-only by contract: the caller pauses feeding while an import runs or
//! a Z-key RAW decode is in flight, and replaces the queue on every photo
//! change, which cancels queued jobs that are no longer neighbours.

use crate::io::thumbnails;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

/// One preload request.
#[derive(Debug, Clone)]
pub struct PreloadJob {
    pub photo_id: i64,
    pub hash: String,
    /// Source path (unused for decoding — the 1920px disk cache is keyed by
    /// hash — kept for logging).
    #[allow(dead_code)]
    pub path: String,
}

/// A decoded preview (or None when the disk cache file was missing).
pub struct PreloadDone {
    pub photo_id: i64,
    pub hash: String,
    pub image: Option<eframe::egui::ColorImage>,
}

pub struct PreloadWorker {
    queue: Arc<(Mutex<VecDeque<PreloadJob>>, Condvar)>,
    rx: Receiver<PreloadDone>,
    threads: Vec<JoinHandle<()>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl Default for PreloadWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl PreloadWorker {
    /// A single worker: preloading is IO+decode bound and must not compete
    /// with the import/decode pools.
    pub fn new() -> Self {
        let (queue, cv) = (Mutex::new(VecDeque::new()), Condvar::new());
        let queue = Arc::new((queue, cv));
        let (tx, rx) = channel();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let q = Arc::clone(&queue);
        let c = Arc::clone(&cancel);
        let thread = std::thread::spawn(move || Self::run_loop(q, tx, c));
        PreloadWorker {
            queue,
            rx,
            threads: vec![thread],
            cancel,
        }
    }

    fn run_loop(
        queue: Arc<(Mutex<VecDeque<PreloadJob>>, Condvar)>,
        tx: Sender<PreloadDone>,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    ) {
        let (lock, cv) = &*queue;
        let mut guard = match lock.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        loop {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if let Some(job) = guard.pop_front() {
                drop(guard);
                let pv = thumbnails::preview_path(&job.hash);
                let image = std::fs::read(&pv)
                    .ok()
                    .and_then(|bytes| image::load_from_memory(&bytes).ok())
                    .map(|img| {
                        let rgba = img.to_rgba8();
                        eframe::egui::ColorImage::from_rgba_unmultiplied(
                            [rgba.width() as usize, rgba.height() as usize],
                            rgba.as_raw(),
                        )
                    });
                let _ = tx.send(PreloadDone {
                    photo_id: job.photo_id,
                    hash: job.hash,
                    image,
                });
                guard = match lock.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                continue;
            }
            guard = match cv.wait(guard) {
                Ok(g) => g,
                Err(_) => return,
            };
        }
    }

    /// Replace the pending queue (PRD 9.5: after a photo change the queue is
    /// rebuilt from the new ±1 window, cancelling stale queued jobs). Jobs
    /// already decoded are filtered by the caller.
    pub fn set_queue(&self, jobs: Vec<PreloadJob>) {
        let (lock, cv) = &*self.queue;
        if let Ok(mut q) = lock.lock() {
            *q = jobs.into_iter().collect();
            drop(q);
        }
        cv.notify_one();
    }

    /// True when a job with this (photo_id, hash) is still waiting in the
    /// queue (in-flight jobs are not tracked — they finish within ~50ms).
    pub fn is_queued(&self, key: &(i64, String)) -> bool {
        let (lock, _) = &*self.queue;
        lock.lock()
            .map(|q| q.iter().any(|j| (j.photo_id, j.hash.clone()) == *key))
            .unwrap_or(false)
    }

    /// Non-blocking drain of decoded previews.
    pub fn poll(&mut self) -> Vec<PreloadDone> {
        let mut out = Vec::new();
        while let Ok(done) = self.rx.try_recv() {
            out.push(done);
        }
        out
    }
}

impl Drop for PreloadWorker {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // Wake the worker so it notices the cancel flag (Arc::get_mut would
        // always fail here — the worker thread holds a clone of the Arc, and
        // without the notify the join below would hang forever).
        let (_, cv) = &*self.queue;
        cv.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Existence helper kept here so callers don't import thumbnails for it.
pub fn preview_cache_exists(hash: &str) -> bool {
    Path::new(&thumbnails::preview_path(hash)).exists()
}
