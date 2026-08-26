//! Background thumbnail/preview generation worker pool (PRD 3.1, 9.3).
//!
//! The UI never blocks on thumbnail generation: when a photo's cache is missing
//! it shows a placeholder and the photo id is queued here. A worker thread
//! generates the thumbnail + preview to disk and reports completion back; the
//! UI then invalidates the texture cache entry so it reloads next frame.

use crate::io::thumbnails;
use std::collections::HashSet;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A generation job.
#[derive(Debug, Clone)]
struct Job {
    photo_id: i64,
    hash: String,
    path: String,
}

/// Completion notification back to the UI.
pub enum ThumbEvent {
    Done { photo_id: i64, hash: String },
}

pub struct ThumbWorker {
    tx: Sender<Job>,
    rx: Receiver<ThumbEvent>,
    pending: HashSet<(i64, String)>,
    thread: Option<std::thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
}

impl Default for ThumbWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl ThumbWorker {
    pub fn new() -> Self {
        let (job_tx, job_rx) = channel::<Job>();
        let (ev_tx, ev_rx) = channel::<ThumbEvent>();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_t = Arc::clone(&cancel);
        let thread = std::thread::spawn(move || {
            Self::run_loop(job_rx, ev_tx, cancel_t);
        });
        ThumbWorker {
            tx: job_tx,
            rx: ev_rx,
            pending: HashSet::new(),
            thread: Some(thread),
            cancel,
        }
    }

    fn run_loop(
        job_rx: Receiver<Job>,
        ev_tx: Sender<ThumbEvent>,
        cancel: Arc<AtomicBool>,
    ) {
        use std::sync::mpsc::RecvTimeoutError;
        loop {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            // Timeout lets the loop notice `cancel` so `Drop` can join cleanly.
            match job_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(job) => {
                    let src = Path::new(&job.path);
                    // Best-effort generate both caches (single decode; raw files
                    // may fall back to a slow full decode internally).
                    let _ = thumbnails::generate_caches(src, &job.hash, 1.0);
                    let _ = ev_tx.send(ThumbEvent::Done {
                        photo_id: job.photo_id,
                        hash: job.hash,
                    });
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    /// Enqueue a photo for generation if it isn't already pending or generated.
    pub fn enqueue(&mut self, photo_id: i64, hash: &str, path: &str) {
        if hash.is_empty() {
            return;
        }
        let key = (photo_id, hash.to_string());
        if self.pending.contains(&key) {
            return;
        }
        self.pending.insert(key);
        let _ = self.tx.send(Job {
            photo_id,
            hash: hash.to_string(),
            path: path.to_string(),
        });
    }

    /// Non-blocking drain of completion events. Returns list of (photo_id, hash).
    pub fn poll(&mut self) -> Vec<(i64, String)> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            let ThumbEvent::Done { photo_id, hash } = ev;
            self.pending.remove(&(photo_id, hash.clone()));
            out.push((photo_id, hash));
        }
        out
    }

    /// True if a job for (photo_id, hash) is in flight.
    pub fn is_pending(&self, photo_id: i64, hash: &str) -> bool {
        self.pending.contains(&(photo_id, hash.to_string()))
    }
}

impl Drop for ThumbWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
