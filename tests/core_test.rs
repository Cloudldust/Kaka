//! Core M1 pipeline tests: DB init, add-mode import, dedup, thumbnails.

use kaka::app::import;
use kaka::db;
use kaka::db::Db;
use kaka::io::{scanner, thumbnails};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_root() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("kaka_test_{}_{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_jpeg(path: &Path, color: [u8; 3]) {
    let img = image::RgbImage::from_fn(64, 64, |_x, _y| {
        image::Rgb([color[0], color[1], color[2]])
    });
    img.save(path).unwrap();
}

#[test]
fn db_init_and_integrity() {
    let root = temp_root();
    let db_path = root.join("a.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();
    assert!(db.integrity_check().unwrap());
    assert_eq!(db::photos::status_counts(&db, "").unwrap().total, 0);
    // Migration to current version is a no-op but should not error.
    db::schema::migrate(&mut db).unwrap();
}

#[test]
fn workspace_state_roundtrip_and_crash_marker() {
    let root = temp_root();
    let db_path = root.join("b.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    // Crash marker: default false, mark -> true, clear -> false.
    assert!(!db::workspace::crash_marker(&db).unwrap());
    db::workspace::mark_crash(&db).unwrap();
    assert!(db::workspace::crash_marker(&db).unwrap());
    db::workspace::clear_crash(&db).unwrap();
    assert!(!db::workspace::crash_marker(&db).unwrap());

    // Save + load a workspace.
    db::workspace::save(
        &db,
        &kaka::model::WorkspaceState {
            current_folder_path: Some("C:/photos".into()),
            current_index: 3,
            current_sort: "capture_time_asc".into(),
            filter_json: None,
            last_selected_id: Some(7),
            last_save_time: String::new(),
            last_crash_marker: false,
            recent_folders_json: None,
        },
    )
    .unwrap();
    let loaded = db::workspace::load(&db).unwrap().unwrap();
    assert_eq!(loaded.current_folder_path.as_deref(), Some("C:/photos"));
    assert_eq!(loaded.current_index, 3);
    assert_eq!(loaded.last_selected_id, Some(7));
}

#[test]
fn scanner_filters_supported_formats() {
    let root = temp_root();
    let src = root.join("src");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    make_jpeg(&src.join("a.JPG"), [255, 0, 0]);
    // Unsupported + hidden + sidecar (must share the photo stem).
    std::fs::write(src.join("vid.mp4"), b"x").unwrap();
    std::fs::write(src.join("note.txt"), b"x").unwrap();
    std::fs::write(src.join("Thumbs.db"), b"x").unwrap();
    std::fs::write(src.join("a.xmp"), b"x").unwrap();
    make_jpeg(&src.join("sub/b.JPEG"), [0, 255, 0]);

    let items = scanner::scan_folder(&src, scanner::ScanOptions { recursive: true }).unwrap();
    let names: Vec<String> = items.iter().map(|i| i.filename.clone()).collect();
    assert_eq!(names, vec!["a.JPG".to_string(), "b.JPEG".to_string()]);
    assert_eq!(items[0].has_sidecar, true);
}

#[test]
fn add_mode_import_dedup_and_insert() {
    let root = temp_root();
    let db_path = root.join("c.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("import");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [10, 20, 30]);
    make_jpeg(&src.join("DSC_0002.JPG"), [40, 50, 60]);

    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    let outcome = import::add_mode_import(&mut db, &src, true, true, &mut prog).unwrap();
    assert_eq!(outcome.added, 2);
    assert_eq!(outcome.skipped_existing, 0);
    assert_eq!(outcome.failed, 0);

    // Second run: same files, dedup should skip both.
    let outcome2 = import::add_mode_import(&mut db, &src, true, true, &mut prog).unwrap();
    assert_eq!(outcome2.added, 0);
    assert_eq!(outcome2.skipped_existing, 2);

    let items = db::photos::list_items_in_folder(&db, &src.to_string_lossy(), kaka::model::SortOrder::CaptureTimeAsc).unwrap();
    assert_eq!(items.len(), 2);
    // All start untreated; capture time used mtime fallback (no EXIF in generated files).
    for p in &items {
        assert_eq!(p.status, kaka::model::Status::Untreated);
        assert!(p.thumb_hash.is_some());
        let full = db::photos::get_photo(&db, p.id).unwrap().unwrap();
        assert_eq!(full.capture_time_source, "mtime_fallback");
    }

    // Unique constraint: re-inserting a duplicate returns None.
    let dup = items[0].clone();
    let photo = kaka::model::Photo {
        id: 0,
        original_filename: dup.original_filename,
        file_size: dup.file_size,
        capture_time: dup.capture_time,
        current_path: dup.current_path,
        folder_path: dup.folder_path,
        status: kaka::model::Status::Untreated,
        thumb_hash: dup.thumb_hash,
        decode_failed: false,
        preview_only: false,
        rotation_override: 0,
        exif_orientation: 1,
        pair_group_id: None,
        iso: None,
        aperture: None,
        shutter_speed: None,
        focal_length: None,
        camera_model: None,
        lens_model: None,
        capture_time_source: "mtime_fallback".into(),
        import_time: String::new(),
        last_access_time: String::new(),
        marked_delete_time: None,
        marked_review_time: None,
    };
    assert!(db::photos::insert_photo(&db, &photo).unwrap().is_none());
}

#[test]
fn thumbnail_generation() {
    let root = temp_root();
    let src = root.join("thumb");
    std::fs::create_dir_all(&src).unwrap();
    let jpg = src.join("img.jpg");
    make_jpeg(&jpg, [100, 150, 200]);
    let dest = src.join("out.jpg");
    let ok = thumbnails::generate_thumbnail(&jpg, &dest, 256, 80).unwrap();
    assert!(ok);
    assert!(dest.exists());
    let img = image::open(&dest).unwrap();
    assert!(img.width() >= 1 && img.height() >= 1);
    let max_edge = img.width().max(img.height());
    assert!(max_edge <= 256);
}

#[test]
fn copy_mode_flat_conflicts_and_dedup() {
    let root = temp_root();
    let db_path = root.join("e.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("card");
    std::fs::create_dir_all(src.join("subA")).unwrap();
    std::fs::create_dir_all(src.join("subB")).unwrap();
    make_jpeg(&src.join("subA/DSC_0001.JPG"), [10, 10, 10]);
    // A different-size image so the three-element dedup treats it as a distinct
    // file (same name but different byte size), forcing a flat-mode _dup suffix.
    let img_b = image::RgbImage::from_fn(32, 32, |_x, _y| image::Rgb([200, 200, 200]));
    img_b.save(&src.join("subB/DSC_0001.JPG")).unwrap();
    make_jpeg(&src.join("subA/DSC_0002.JPG"), [30, 30, 30]);

    let target = root.join("out");
    let opts = kaka::app::copy::CopyOptions {
        target_dir: target.to_string_lossy().into_owned(),
        org_mode: kaka::app::copy::OrgMode::Flat,
        recursive: true,
        dedup: true,
        clear_card: false,
    };
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    let out = kaka::app::copy::copy_mode_import(&mut db, &src, &opts, false, 0, &mut prog).unwrap();
    assert_eq!(out.copied, 3, "all three photos should be copied");
    assert_eq!(out.failed, 0);

    // Flat mode: two DSC_0001.JPGs collide -> one becomes _dup1.
    let mut names: Vec<String> = std::fs::read_dir(&target).unwrap()
        .flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert!(names.contains(&"DSC_0001.JPG".to_string()));
    assert!(names.contains(&"DSC_0001_dup1.JPG".to_string()), "flat conflict should add _dup1; got {names:?}");
    assert!(names.contains(&"DSC_0002.JPG".to_string()));

    // DB should have 3 records with current_path pointing into the target.
    let items = db::photos::list_items_in_folder(&db, &target.to_string_lossy(), kaka::model::SortOrder::CaptureTimeAsc).unwrap();
    assert_eq!(items.len(), 3);
    for p in &items {
        assert!(p.current_path.starts_with(&target.to_string_lossy().into_owned()));
    }

    // Re-import: dedup skips all.
    let out2 = kaka::app::copy::copy_mode_import(&mut db, &src, &opts, false, 0, &mut prog).unwrap();
    assert_eq!(out2.copied, 0);
    assert_eq!(out2.skipped_existing, 3);
}

#[test]
fn copy_mode_structure_preserves_relative_dirs() {
    let root = temp_root();
    let db_path = root.join("f.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("card");
    std::fs::create_dir_all(src.join("100NIKON")).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [1, 2, 3]);
    make_jpeg(&src.join("100NIKON/DSC_0002.JPG"), [4, 5, 6]);

    let target = root.join("out");
    let opts = kaka::app::copy::CopyOptions {
        target_dir: target.to_string_lossy().into_owned(),
        org_mode: kaka::app::copy::OrgMode::Structure,
        recursive: true,
        dedup: true,
        clear_card: false,
    };
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    let out = kaka::app::copy::copy_mode_import(&mut db, &src, &opts, false, 0, &mut prog).unwrap();
    assert_eq!(out.copied, 2);
    assert!(target.join("DSC_0001.JPG").exists());
    assert!(target.join("100NIKON/DSC_0002.JPG").exists(), "structure mode should keep relative dirs");
}

#[test]
fn session_journal_lifecycle() {
    // Validate the ImportSession serialization round-trip.
    let sess = kaka::app::session::ImportSession {
        session_id: "test_123".into(),
        mode: "copy".into(),
        source: "C:/src".into(),
        target: "D:/dst".into(),
        org_mode: "flat".into(),
        recursive: true,
        dedup: true,
        created_at: "now".into(),
        completed: false,
        abandoned: false,
        total: 10,
        done: 4,
    };
    // start/write are I/O against %APPDATA%; we only assert the struct is sane.
    assert!(sess.session_id.starts_with("test_"));
    assert_eq!(sess.total, 10);
    assert_eq!(sess.done, 4);
    assert!(!sess.completed && !sess.abandoned);
    let json = serde_json::to_string(&sess).unwrap();
    let back: kaka::app::session::ImportSession = serde_json::from_str(&json).unwrap();
    assert_eq!(back.session_id, "test_123");
    assert_eq!(back.done, 4);
}

#[test]
fn raw_without_preview_degrades_gracefully() {
    let root = temp_root();
    let fake_raw = root.join("x.nef");
    // A few bytes that are not a decodable TIFF/JPEG; embedded preview none.
    std::fs::write(&fake_raw, b"II*\x00\x08\x00\x00\x00 this is not a real NEF").unwrap();
    let dest = root.join("thumb.jpg");
    let ok = thumbnails::generate_thumbnail(&fake_raw, &dest, 256, 80).unwrap();
    assert!(!ok, "un-decodable RAW should return Ok(false) without crashing");
    assert!(!dest.exists());
}

#[test]
fn async_thumb_worker_enqueue_and_finish() {
    use kaka::app::thumbs::ThumbWorker;
    let root = temp_root();
    let jpg = root.join("w.jpg");
    make_jpeg(&jpg, [7, 8, 9]);

    let mut worker = ThumbWorker::new();
    let hash = "testhash";
    worker.enqueue(99, hash, &jpg.to_string_lossy());
    assert!(worker.is_pending(99, hash), "enqueue should mark pending");
    // A duplicate enqueue must not double-queue (still one pending entry).
    worker.enqueue(99, hash, &jpg.to_string_lossy());

    // Wait for the worker to finish the job (it generates to the disk cache).
    // The pending entry is only cleared when the event is drained by poll().
    let mut finished = false;
    let mut drained: Vec<(i64, String)> = Vec::new();
    for _ in 0..300 {
        for ev in worker.poll() {
            if ev.0 == 99 && ev.1 == hash {
                finished = true;
            }
            drained.push(ev);
        }
        if finished {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(finished, "worker should complete the generation job");
    assert_eq!(drained, vec![(99, hash.to_string())]);
}

#[test]
fn copy_mode_date_subfolder() {
    let root = temp_root();
    let db_path = root.join("g.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("card");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [9, 9, 9]);

    let target = root.join("out");
    let opts = kaka::app::copy::CopyOptions {
        target_dir: target.to_string_lossy().into_owned(),
        org_mode: kaka::app::copy::OrgMode::Date,
        recursive: true,
        dedup: true,
        clear_card: false,
    };
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    let out = kaka::app::copy::copy_mode_import(&mut db, &src, &opts, false, 0, &mut prog).unwrap();
    assert_eq!(out.copied, 1);
    // No EXIF -> capture via mtime (today), so a date subfolder is created.
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert!(
        target.join(&today).join("DSC_0001.JPG").exists(),
        "date mode should place the file under {today}; found: {:?}",
        std::fs::read_dir(&target).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>())
    );
}

#[test]
fn copy_mode_resume_progress_continues_from_base() {
    let root = temp_root();
    let db_path = root.join("h.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("card");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [1, 1, 1]);
    make_jpeg(&src.join("DSC_0002.JPG"), [2, 2, 2]);

    let target = root.join("out");
    let opts = kaka::app::copy::CopyOptions {
        target_dir: target.to_string_lossy().into_owned(),
        org_mode: kaka::app::copy::OrgMode::Structure,
        recursive: true,
        dedup: true,
        clear_card: false,
    };
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    let out = kaka::app::copy::copy_mode_import(&mut db, &src, &opts, false, 0, &mut prog).unwrap();
    assert_eq!(out.copied, 2);

    // Add a third file that is not yet imported.
    make_jpeg(&src.join("DSC_0003.JPG"), [3, 3, 3]);

    // Resume with base=2: the two already-imported files are dedup-skipped and
    // the copy progress must continue from 2 (not restart at 0).
    let mut copy_phase_max = 0usize;
    {
        let mut prog = |phase: &str, done: usize, _t: usize, _n: &str| -> bool {
            if phase == "拷贝" {
                copy_phase_max = copy_phase_max.max(done);
            }
            true
        };
        let out2 = kaka::app::copy::copy_mode_import(&mut db, &src, &opts, true, 2, &mut prog).unwrap();
        assert_eq!(out2.copied, 1, "only the new file should be copied");
        assert_eq!(out2.skipped_existing, 2, "the two existing files are skipped as duplicates");
        assert_eq!(copy_phase_max, 3, "copy progress should continue from base=2 to 3");
    }
}

#[test]
fn m3_undo_redo_and_selection() {
    use kaka::app::state::AppState;
    use kaka::model::{AppConfig, Status};

    let root = temp_root();
    let db_path = root.join("m3.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("photos");
    std::fs::create_dir_all(&src).unwrap();
    for i in 0..4 {
        make_jpeg(&src.join(format!("DSC_{i:04}.JPG")), [i as u8, 40, 80]);
    }
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog).unwrap();

    let mut app = AppState::new(db, AppConfig::default());
    app.open_workspace(&src.to_string_lossy(), kaka::model::SortOrder::FilenameAsc).unwrap();
    assert_eq!(app.ws.items.len(), 4);

    // Single Q on the first photo records history for undo.
    let first_id = app.ws.items[0].id;
    assert!(app.set_status_current(Status::Delete, true).unwrap());
    assert!(app.ws.items[0].status == Status::Delete);
    assert_eq!(app.undo_stack.len(), 1);

    // Undo reverts it; redo re-applies it.
    assert!(app.undo());
    assert!(app.ws.items[0].status == Status::Untreated);
    assert_eq!(app.redo_stack.len(), 1);
    assert!(app.redo());
    assert!(app.ws.items[0].status == Status::Delete);

    // Select via click: plain = single, ctrl = toggle, shift = range.
    app.select_click(1, false, false); // select photos[1] only, make current
    assert_eq!(app.ws.selected_count(), 1);
    assert!(app.ws.selection.contains(&app.ws.items[1].id));
    app.select_click(2, true, false); // ctrl toggle adds photos[2]
    assert_eq!(app.ws.selected_count(), 2);
    app.select_click(3, true, false); // ctrl toggle adds photos[3]
    assert_eq!(app.ws.selected_count(), 3);

    // Batch apply Reviewed to the selection (not undoable).
    let n = app.set_status_selected(Status::Reviewed).unwrap();
    assert_eq!(n, 3);
    assert_eq!(app.undo_stack.len(), 1, "batch must NOT enter the undo stack");
    for p in &app.ws.items[1..] {
        assert_eq!(p.status, Status::Reviewed);
    }

    // Clear selection.
    assert!(app.clear_selection());
    assert_eq!(app.ws.selected_count(), 0);
}

/// Build a minimal little-endian TIFF whose IFD0 carries a single embedded JPEG
/// preview referenced by JPEGInterchangeFormat/JPEGInterchangeFormatLength.
/// This exercises the same path a TIFF-based RAW (NEF/ARW/CR2/DNG/ORF…) uses.
fn tiff_with_embedded_jpeg(jpeg: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // TIFF header: "II", magic 0x002A, IFD0 at offset 8.
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&8u32.to_le_bytes());

    // IFD0: 2 entries.
    out.extend_from_slice(&2u16.to_le_bytes());
    // Entry: tag, type(LONG=4), count, value.
    let jpeg_offset = 38u32; // 8 (header) + 30 (IFD size)
    let jpeg_len = jpeg.len() as u32;
    // tag 0x0201 = JPEGInterchangeFormat
    out.extend_from_slice(&0x0201u16.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes()); // LONG
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&jpeg_offset.to_le_bytes());
    // tag 0x0202 = JPEGInterchangeFormatLength
    out.extend_from_slice(&0x0202u16.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&jpeg_len.to_le_bytes());
    // next IFD offset = 0.
    out.extend_from_slice(&0u32.to_le_bytes());
    // Embedded JPEG data.
    out.extend_from_slice(jpeg);
    out
}

#[test]
fn extract_embedded_preview_from_tiff_container() {
    // Build a real small JPEG to act as the "embedded preview".
    let root = temp_root();
    let jpg = root.join("pv.jpg");
    make_jpeg(&jpg, [20, 60, 120]);
    let jpeg_bytes = std::fs::read(&jpg).unwrap();

    let tiff = tiff_with_embedded_jpeg(&jpeg_bytes);
    let raw_path = root.join("fake.nef");
    std::fs::write(&raw_path, &tiff).unwrap();

    let extracted = kaka::io::exif::extract_embedded_preview(&raw_path);
    assert!(extracted.is_some(), "embedded preview should be extractable from the TIFF container");
    let bytes = extracted.unwrap();
    assert_eq!(bytes, jpeg_bytes, "extracted bytes should match the embedded JPEG");
    // And it should be a decodable image (image crate).
    assert!(image::load_from_memory(&bytes).is_ok());
}

#[test]
fn embedded_icon_decodes() {
    // Mirrors the runtime `load_icon()` path: the ICO is baked in at build time
    // and decoded with the image crate to make the window/taskbar icon.
    let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/KAKA.ico"));
    let img = image::load_from_memory(bytes).expect("KAKA.ico should be decodable");
    let rgba = img.to_rgba8();
    assert!(rgba.width() >= 16 && rgba.height() >= 16);
    assert!(!rgba.as_raw().is_empty());
    // At least one opaque pixel (so the icon is visible, not all-transparent).
    assert!(rgba.as_raw().chunks_exact(4).any(|p| p[3] != 0));
}

#[test]
fn import_then_open_workspace_across_connections() {
    // Simulates the real flow: an import thread writes to the DB with its own
    // connection, then the main connection loads the workspace and generates a
    // thumbnail for the first photo. This exercises the same cross-connection
    // read-after-write and path-matching the GUI depends on.
    let root = temp_root();
    let db_path = root.join("d.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("photos");
    std::fs::create_dir_all(&src).unwrap();
    for i in 0..28 {
        make_jpeg(&src.join(format!("DSC_{i:04}.JPG")), [i as u8, 60, 90]);
    }

    // Import with a trailing separator, to guard the prefix-matching path.
    let src_str = src.to_string_lossy().into_owned();
    let bordered = format!("{src_str}\\");
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    let outcome = import::add_mode_import(&mut db, std::path::Path::new(&bordered), true, true, &mut prog).unwrap();
    assert_eq!(outcome.added, 28);

    // Open the workspace using a DIFFERENT connection (as the GUI thread does).
    let db2 = Db::open(&db_path).unwrap();
    // Simulate open_workspace: query items + counts by prefix.
    let items = db::photos::list_items_in_folder(&db2, &src.to_string_lossy(), kaka::model::SortOrder::CaptureTimeAsc).unwrap();    assert_eq!(items.len(), 28, "workspace should see all imported photos");
    assert_eq!(db::photos::status_counts(&db2, &src.to_string_lossy()).unwrap().total, 28);

    // Generate + validate a thumbnail for the first photo (the preview path).
    let first = &items[0];
    let hash = first.thumb_hash.clone().unwrap();
    let cached = thumbnails::ensure_thumbnail(std::path::Path::new(&first.current_path), &hash, 1.0).unwrap();
    assert!(cached.is_some(), "JPG thumbnail should generate");
    let img = image::open(cached.unwrap()).unwrap();
    assert!(img.width() >= 1 && img.height() >= 1);

    // Large preview (long edge <= 1920) should also generate for JPG sources.
    let pv = thumbnails::ensure_preview(std::path::Path::new(&first.current_path), &hash).unwrap();
    assert!(pv.is_some(), "JPG preview should generate");
    let pimg = image::open(pv.unwrap()).unwrap();
    assert!(pimg.width().max(pimg.height()) <= 1920);
}
