//! Removable media (SD card) hot-plug detector (PRD 6.2 自动触发).
//!
//! A background thread polls the removable drive letters every ~2.5s. When a
//! new removable drive appears, it emits an `Inserted` event so the UI can open
//! the import dialog with that drive as the source. Removal is just tracked
//! (no event, so it can detect a subsequent re-insert).

use std::collections::HashSet;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const DRIVE_REMOVABLE: u32 = 2;
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(2500);

pub enum CardEvent {
    /// A new removable drive (SD card) appeared. The drive letter, e.g. 'E'.
    Inserted(char),
}

pub struct CardDetector {
    rx: Receiver<CardEvent>,
    thread: Option<std::thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
}

impl Default for CardDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl CardDetector {
    pub fn new() -> Self {
        let (tx, rx) = channel::<CardEvent>();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_t = Arc::clone(&cancel);
        let thread = std::thread::spawn(move || {
            Self::run_loop(tx, cancel_t);
        });
        CardDetector {
            rx,
            thread: Some(thread),
            cancel,
        }
    }

    fn run_loop(tx: Sender<CardEvent>, cancel: Arc<AtomicBool>) {
        // Track the current removable drives so we can detect a new one.
        let mut known: HashSet<char> = snapshot();
        while !cancel.load(Ordering::SeqCst) {
            std::thread::sleep(POLL_INTERVAL);
            let now = snapshot();
            for c in &now {
                if !known.contains(c) {
                    let _ = tx.send(CardEvent::Inserted(*c));
                }
            }
            known = now;
        }
    }

    /// Non-blocking poll of the pending insertion event.
    pub fn poll(&self) -> Option<CardEvent> {
        self.rx.try_recv().ok()
    }
}

impl Drop for CardDetector {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// True when `path` lies on a removable drive (e.g. an SD card reader mounted
/// as a drive letter). Used to gate the 清空存储卡 option (PRD 6.3).
pub fn is_removable_source(path: &std::path::Path) -> bool {
    let p = match path.to_str() {
        Some(p) => p,
        None => return false,
    };
    // Only drive-letter absolute paths like "E:\..." or "E:/..." matter.
    let b = p.as_bytes();
    if b.len() < 3 || b[1] != b':' {
        return false;
    }
    let letter = (b[0].to_ascii_uppercase()) as char;
    let root: Vec<u16> = format!("{letter}:\\").encode_utf16().collect();
    unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) == DRIVE_REMOVABLE }
}

/// Snapshot the set of removable drive letters currently present.
fn snapshot() -> HashSet<char> {
    let mut out = HashSet::new();
    unsafe {
        let mask = windows_sys::Win32::Storage::FileSystem::GetLogicalDrives();
        for i in 0..26u8 {
            if mask & (1u32 << i) == 0 {
                continue;
            }
            let letter = (b'A' + i) as char;
            let path: Vec<u16> = format!("{letter}:\\").encode_utf16().collect();
            let t = windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(path.as_ptr());
            if t == DRIVE_REMOVABLE {
                out.insert(letter);
            }
        }
    }
    out
}
