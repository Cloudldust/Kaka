//! Background thumbnail/preview generation worker pool (PRD 3.1, 9.3).
//!
//! The UI never blocks on thumbnail generation: when a photo's cache is missing
//! it shows a placeholder and the photo id is queued here. A small pool of worker
//! threads generates thumbnails + previews to disk concurrently and reports
//! completion back; the UI then invalidates the texture cache entry so it reloads
//! next frame. The pool is capped so it does not starve the main IO/UI.

use crate::io::thumbnails;
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

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

/// Shared job queue (mutex + condvar) so several workers can drain jobs in
/// parallel while the UI enqueues without blocking.
type JobQueue = Arc<(Mutex<VecDeque<Job>>, Condvar)>;

/// Number of concurrent generation workers. Capped so thumbnail IO/decode does
/// not starve the UI thread or the source disk.
const THUMB_WORKERS: usize = 4;

pub struct ThumbWorker {
    queue: JobQueue,
    rx: Receiver<ThumbEvent>,
    pending: HashSet<(i64, String)>,
    threads: Vec<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
}

impl Default for ThumbWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl ThumbWorker {
    pub fn new() -> Self {
        let (ev_tx, ev_rx) = channel::<ThumbEvent>();
        let cancel = Arc::new(AtomicBool::new(false));
        let queue: JobQueue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));

        let mut threads = Vec::with_capacity(THUMB_WORKERS);
        for _ in 0..THUMB_WORKERS {
            let queue = Arc::clone(&queue);
            let ev_tx = ev_tx.clone();
            let cancel = Arc::clone(&cancel);
            threads.push(std::thread::spawn(move || {
                Self::run_loop(queue, ev_tx, cancel);
            }));
        }

        ThumbWorker {
            queue,
            rx: ev_rx,
            pending: HashSet::new(),
            threads,
            cancel,
        }
    }

    fn run_loop(queue: JobQueue, ev_tx: Sender<ThumbEvent>, cancel: Arc<AtomicBool>) {
        let (lock, cv) = &*queue;
        let mut guard = match lock.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        loop {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            if let Some(job) = guard.pop_front() {
                // Drop the lock while generating so other workers can also run.
                drop(guard);
                let src = Path::new(&job.path);
                // Best-effort generate both caches (single decode; raw files may
                // fall back to a slow full decode internally).
                let _ = thumbnails::generate_caches(src, &job.hash, 1.0);
                let _ = ev_tx.send(ThumbEvent::Done {
                    photo_id: job.photo_id,
                    hash: job.hash,
                });
                guard = match lock.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                continue;
            }
            // Nothing to do: wait for a job or a cancel signal.
            guard = match cv.wait(guard) {
                Ok(g) => g,
                Err(_) => return,
            };
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
        let (lock, cv) = &*self.queue;
        if let Ok(mut q) = lock.lock() {
            q.push_back(Job {
                photo_id,
                hash: hash.to_string(),
                path: path.to_string(),
            });
            drop(q);
        }
        cv.notify_one();
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
        // Wake every worker so they notice the cancel / see an empty queue.
        let (_, cv) = &*self.queue;
        cv.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}
