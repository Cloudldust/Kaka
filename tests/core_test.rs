//! Core M1 pipeline tests: DB init, add-mode import, dedup, thumbnails.

use kaka::app::import;
use kaka::db;
use kaka::db::Db;
use kaka::io::{scanner, thumbnails};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_root() -> PathBuf {
    // Copy-mode tests trigger the disk-space pre-check; they must not depend
    // on how full the temp drive happens to be.
    kaka::app::copy::disable_disk_guard_for_tests();
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
    let outcome = import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();
    assert_eq!(outcome.added, 2);
    assert_eq!(outcome.skipped_existing, 0);
    assert_eq!(outcome.failed, 0);

    // Second run: same files, dedup should skip both.
    let outcome2 = import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();
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
        aperture_num: None,
        shutter_num: None,
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
        pair_threshold_secs: 5,
    };
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    let out = kaka::app::copy::copy_mode_import(&mut db, &src, &opts, false, 0, &mut prog, None).unwrap();
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
    let out2 = kaka::app::copy::copy_mode_import(&mut db, &src, &opts, false, 0, &mut prog, None).unwrap();
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
        pair_threshold_secs: 5,
    };
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    let out = kaka::app::copy::copy_mode_import(&mut db, &src, &opts, false, 0, &mut prog, None).unwrap();
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
        for ev in worker.poll_limited(100) {
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
        pair_threshold_secs: 5,
    };
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    let out = kaka::app::copy::copy_mode_import(&mut db, &src, &opts, false, 0, &mut prog, None).unwrap();
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
        pair_threshold_secs: 5,
    };
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    let out = kaka::app::copy::copy_mode_import(&mut db, &src, &opts, false, 0, &mut prog, None).unwrap();
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
        let out2 = kaka::app::copy::copy_mode_import(&mut db, &src, &opts, true, 2, &mut prog, None).unwrap();
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
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();

    let mut app = AppState::new(db, AppConfig::default());
    app.open_workspace(&src.to_string_lossy(), kaka::model::SortOrder::FilenameAsc).unwrap();
    assert_eq!(app.ws.items.len(), 4);

    // Single Q on the first photo records history for undo.
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

#[test]
fn histogram_computes_overflow_ratios() {
    use kaka::io::histogram::Histogram;
    // Solid grey: histogram concentrates mid-tones, no clipping.
    let grey = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 64, |_, _| image::Rgb([128u8, 128u8, 128u8])));
    let h = Histogram::from_image(&grey);
    assert_eq!(h.total, 64 * 64);
    assert!(h.l[128] > 0);
    assert_eq!(h.black_ratio(), 0.0);
    assert_eq!(h.white_ratio(), 0.0);

    // Pure black → heavy black clipping, no white clipping.
    let black = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 64, |_, _| image::Rgb([0u8, 0u8, 0u8])));
    let h = Histogram::from_image(&black);
    assert!(h.black_ratio() > 0.9);
    assert_eq!(h.white_ratio(), 0.0);

    // Pure white → heavy white clipping.
    let white = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 64, |_, _| image::Rgb([255u8, 255u8, 255u8])));
    let h = Histogram::from_image(&white);
    assert!(h.white_ratio() > 0.9);
}

#[test]
fn advanced_filter_status_format_missing() {
    use kaka::db::photos::list_items_filtered;
    use kaka::model::Filter;

    let root = temp_root();
    let db_path = root.join("f.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("photos");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [10, 20, 30]);
    make_jpeg(&src.join("DSC_0002.JPG"), [40, 50, 60]);
    make_jpeg(&src.join("DSC_0003.JPG"), [70, 80, 90]);
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();

    let folder = src.to_string_lossy();
    let items = db::photos::list_items_in_folder(&db, &folder, kaka::model::SortOrder::FilenameAsc).unwrap();
    assert_eq!(items.len(), 3);

    // Mark the first one as delete; filter by status.
    let first = &items[0];
    db::photos::set_status(&db, first.id, kaka::model::Status::Delete).unwrap();
    let mut filt = Filter::default();
    filt.statuses = vec![1];
    let out = list_items_filtered(&db, &folder, kaka::model::SortOrder::FilenameAsc, &filt).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].id, first.id);

    // Format filter: JPEG matches all, NEF matches none.
    let mut fmt = Filter::default();
    fmt.formats = vec!["JPG".into()];
    assert_eq!(list_items_filtered(&db, &folder, kaka::model::SortOrder::FilenameAsc, &fmt).unwrap().len(), 3);
    fmt.formats = vec!["NEF".into()];
    assert_eq!(list_items_filtered(&db, &folder, kaka::model::SortOrder::FilenameAsc, &fmt).unwrap().len(), 0);

    // Missing filter: files exist -> none missing; missing-only -> 0.
    let mut miss = Filter::default();
    miss.missing = Some(false);
    assert_eq!(list_items_filtered(&db, &folder, kaka::model::SortOrder::FilenameAsc, &miss).unwrap().len(), 3);
    let mut mm = Filter::default();
    mm.missing = Some(true);
    assert_eq!(list_items_filtered(&db, &folder, kaka::model::SortOrder::FilenameAsc, &mm).unwrap().len(), 0);
}

#[test]
fn export_kept_copy_and_file_list() {
    use kaka::app::copy::OrgMode;
    use kaka::app::export::{ExportFileFormat, export_file_list, export_kept_copy};
    use kaka::model::Status;

    let root = temp_root();
    let db_path = root.join("exp.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("photos");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [10, 20, 30]);
    make_jpeg(&src.join("DSC_0002.JPG"), [40, 50, 60]);
    make_jpeg(&src.join("DSC_0003.JPG"), [70, 80, 90]);
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();

    let folder = src.to_string_lossy();
    // Mark one photo as Delete so it is excluded from the export.
    let items = db::photos::list_items_in_folder(&db, &folder, kaka::model::SortOrder::FilenameAsc).unwrap();
    db::photos::set_status(&db, items[1].id, Status::Delete).unwrap();

    // 12.1: copy kept photos to a flat target dir.
    let target = root.join("out");
    let mut xp = |_n: &str, _d: usize, _t: usize| -> bool { true };
    let out = export_kept_copy(&db, &folder, &target.to_string_lossy(), OrgMode::Flat, false, false, true, &mut xp).unwrap();
    assert_eq!(out.total, 2, "only the two kept photos should be exported");
    assert_eq!(out.copied, 2);
    assert!(target.join("DSC_0001.JPG").exists());
    assert!(target.join("DSC_0003.JPG").exists());
    assert!(!target.join("DSC_0002.JPG").exists(), "the Delete photo must not be copied");

    // 12.2: write a CSV file list of kept photos.
    let csv = root.join("kept.csv");
    let n = export_file_list(&db, &folder, &csv.to_string_lossy(), ExportFileFormat::Csv).unwrap();
    assert_eq!(n, 2);
    let csv_text = std::fs::read_to_string(&csv).unwrap();
    assert!(csv_text.contains("DSC_0001.JPG"));
    assert!(csv_text.contains("DSC_0003.JPG"));
    assert!(!csv_text.contains("DSC_0002.JPG"));
    // BOM + header present.
    assert!(csv_text.starts_with('\u{feff}'));
    assert!(csv_text.contains("original_filename,current_path,status"));

    // 12.2: TXT list = one absolute path per line.
    let txt = root.join("kept.txt");
    let n = export_file_list(&db, &folder, &txt.to_string_lossy(), ExportFileFormat::Txt).unwrap();
    assert_eq!(n, 2);
    let txt_text = std::fs::read_to_string(&txt).unwrap();
    let lines: Vec<&str> = txt_text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].ends_with("DSC_0001.JPG"), "first path line is wrong: {}", lines[0]);
}

#[test]
fn db_manual_backup_restore_and_reset() {
    use kaka::db::schema;

    let root = temp_root();
    let db_path = root.join("bk.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("photos");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [1, 2, 3]);
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();
    assert_eq!(db::photos::count_photos(&db, "").unwrap(), 1);

    // 手动备份.
    let bak = schema::manual_backup(&db).unwrap();
    assert!(bak.exists(), "manual backup file should exist");

    // 改动当前库：删掉照片记录.
    let items = db::photos::list_items_in_folder(&db, &src.to_string_lossy(), kaka::model::SortOrder::FilenameAsc).unwrap();
    db::photos::delete_photo(&db, items[0].id).unwrap();
    assert_eq!(db::photos::count_photos(&db, "").unwrap(), 0);

    // 从备份恢复.
    schema::restore_backup(&mut db, &bak).unwrap();
    assert_eq!(db::photos::count_photos(&db, "").unwrap(), 1, "restored DB should have the photo again");
    assert_eq!(db::photos::status_counts(&db, "").unwrap().total, 1);

    // 放弃新建（reset to fresh）.
    schema::reset_to_fresh(&mut db).unwrap();
    assert_eq!(db::photos::count_photos(&db, "").unwrap(), 0, "fresh DB should be empty");
}

#[test]
fn corrupt_db_still_opens_and_reports_corruption() {
    use kaka::db::Db;
    let root = temp_root();
    let db_path = root.join("corrupt.db");

    // Build a healthy database first (so the file starts as a real SQLite db).
    {
        let mut db = Db::open(&db_path).unwrap();
        db::schema::init(&mut db).unwrap();
        db::schema::migrate(&mut db).unwrap();
        let src = root.join("photos");
        std::fs::create_dir_all(&src).unwrap();
        make_jpeg(&src.join("DSC_0001.JPG"), [1, 2, 3]);
        let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
        import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();
    }

    // Simulate the user editing the file with a text editor: truncate the tail
    // (deleting part of the data) so the btree no longer matches the header.
    {
        let mut data = std::fs::read(&db_path).unwrap();
        data.truncate(data.len() / 2);
        std::fs::write(&db_path, &data).unwrap();
    }

    // Reopening must NOT fail startup: Db::open succeeds and integrity_check
    // reports Ok(false) (corruption) instead of erroring out.
    let db = Db::open(&db_path).unwrap();
    let ok = db.integrity_check().unwrap_or(true);
    assert!(!ok, "a corrupted DB must report !ok, not error");
}

#[test]
fn auto_repair_prefers_manual_backup() {
    use kaka::db::schema;
    let root = temp_root();
    let db_path = root.join("ar.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("photos");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [1, 2, 3]);
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();

    // 手动备份（正是用户报告的场景）.
    let bak = schema::manual_backup(&db).unwrap();
    assert!(
        bak.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().contains("manual_"),
        "backup should be a manual backup: {}",
        bak.display()
    );

    // 关闭连接后截断库文件，模拟损坏.
    drop(db);
    {
        let mut data = std::fs::read(&db_path).unwrap();
        data.truncate(data.len() / 2);
        std::fs::write(&db_path, &data).unwrap();
    }
    let mut db = Db::open(&db_path).unwrap();
    assert!(!db.integrity_check().unwrap_or(true), "db must now be corrupt");

    // 自动修复：应优先用最近的手动备份恢复，而不是新建空库.
    schema::repair_or_reset(&mut db).unwrap();
    assert_eq!(
        db::photos::count_photos(&db, "").unwrap(),
        1,
        "auto repair must restore from the manual backup, not create a fresh DB"
    );
}

#[test]
fn pair_time_threshold_respected() {
    use kaka::app::copy::reconcile_pairs;
    let root = temp_root();
    let db_path = root.join("pt.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    // 同目录同 stem、不同扩展名（模拟 RAW+JPG）。
    let src = root.join("photos");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [1, 2, 3]);
    make_jpeg(&src.join("DSC_0001.JPEG"), [4, 5, 6]);
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();

    let folder = src.to_string_lossy();
    // 拍摄时间相同（同秒 mtime）→ 5s 阈值内应配对。
    reconcile_pairs(&mut db, &folder, 5).unwrap();
    let items = db::photos::list_items_in_folder(&db, &folder, kaka::model::SortOrder::FilenameAsc).unwrap();
    assert!(items.iter().all(|p| p.pair_group_id.is_some()), "same-time same-stem must pair");

    // 把 .JPEG 的拍摄时间 +10s → 超出 5s 阈值，必须解配对。
    let jpeg = items.iter().find(|p| p.original_filename.ends_with(".JPEG")).unwrap();
    db.conn.execute(
        "UPDATE photos SET capture_time = datetime(capture_time, '+10 seconds') WHERE id = ?1",
        rusqlite::params![jpeg.id],
    ).unwrap();
    reconcile_pairs(&mut db, &folder, 5).unwrap();
    let items = db::photos::list_items_in_folder(&db, &folder, kaka::model::SortOrder::FilenameAsc).unwrap();
    assert!(items.iter().all(|p| p.pair_group_id.is_none()), ">5s apart must NOT pair");

    // 阈值放宽到 10s → 重新配对。
    reconcile_pairs(&mut db, &folder, 10).unwrap();
    let items = db::photos::list_items_in_folder(&db, &folder, kaka::model::SortOrder::FilenameAsc).unwrap();
    assert!(items.iter().all(|p| p.pair_group_id.is_some()), "within 10s should pair");

    // 启动级全库重配对（reconcile_pairs_all）在 5s 阈值下再次解配对。
    kaka::app::copy::reconcile_pairs_all(&mut db, 5).unwrap();
    let items = db::photos::list_items_in_folder(&db, &folder, kaka::model::SortOrder::FilenameAsc).unwrap();
    assert!(items.iter().all(|p| p.pair_group_id.is_none()), "reconcile_pairs_all must honor the threshold too");
}

#[test]
fn prescan_pair_respects_time_threshold() {
    use kaka::app::import::prescan_mark;
    let root = temp_root();
    let db_path = root.join("pp.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("photos");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [1, 2, 3]);
    let jpeg = src.join("DSC_0001.JPEG");
    make_jpeg(&jpeg, [4, 5, 6]);
    // 把 .JPEG 的 mtime +10s（prescan 用 mtime 兜底 → 时间差 10s）。
    let t = std::fs::metadata(&jpeg).unwrap().modified().unwrap() + std::time::Duration::from_secs(10);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&jpeg)
        .unwrap()
        .set_modified(t)
        .unwrap();

    let items = prescan_mark(&mut db, &src, true, 5, &mut |_d, _t| true).unwrap().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| i.pair_group.is_none()), "10s apart must not pair in prescan with 5s threshold");

    // 阈值 30s → 应配对。
    let items = prescan_mark(&mut db, &src, true, 30, &mut |_d, _t| true).unwrap().unwrap();
    assert!(items.iter().all(|i| i.pair_group.is_some()), "within 30s should pair in prescan");
}

#[test]
fn e_u_mark_whole_pair_group() {
    use kaka::app::state::AppState;
    use kaka::model::{AppConfig, Status};

    let root = temp_root();
    let db_path = root.join("eu.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("photos");
    std::fs::create_dir_all(&src).unwrap();
    // 真 RAW（image 无法解码 → preview_only=true）+ 同 stem JPG。
    std::fs::write(
        src.join("DSC_0001.NEF"),
        b"II*\x00\x08\x00\x00\x00 this is not a real NEF",
    )
    .unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [1, 2, 3]);
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();
    let folder = src.to_string_lossy();
    kaka::app::copy::reconcile_pairs(&mut db, &folder, 5).unwrap();

    let mut app = AppState::new(db, AppConfig::default());
    app.open_workspace(&folder, kaka::model::SortOrder::FilenameAsc).unwrap();
    // P1-4: 配对合并显示为「同一张」——每组只保留一个代表（优先 RAW）。
    assert_eq!(app.ws.items.len(), 1, "paired RAW+JPG shows as one merged item");
    let rep = app.ws.items[0].clone();
    assert!(rep.original_filename.to_uppercase().ends_with(".NEF"), "merged representative should be the RAW member; got {}", rep.original_filename);
    assert!(rep.pair_group_id.is_some(), "representative keeps its pair_group_id");
    let gid = rep.pair_group_id.unwrap();
    let group_of = |app: &AppState| kaka::db::photos::list_items_by_pair_group(&app.db, gid).unwrap();

    // E（已阅）对当前（合并）张 → 数据库里整组两张都变成已阅。
    app.set_status_current(Status::Reviewed, true).unwrap();
    assert_eq!(group_of(&app).len(), 2);
    assert!(group_of(&app).iter().all(|p| p.status == Status::Reviewed), "E must mark the whole group");

    // U（重置）→ 整组回未处理。
    app.set_status_current(Status::Untreated, true).unwrap();
    assert!(group_of(&app).iter().all(|p| p.status == Status::Untreated), "U must reset the whole group");

    // Q（待删）→ 整组待删，且 pair_group_id 保留（R+J 角标依赖它）。
    app.set_status_current(Status::Delete, true).unwrap();
    assert!(group_of(&app).iter().all(|p| p.status == Status::Delete), "Q must mark the whole group");
    assert!(group_of(&app).iter().all(|p| p.pair_group_id.is_some()), "pair_group_id must survive Q (badge stays)");
}

#[test]
fn search_expression_and_or_not() {
    use kaka::app::state::eval_search;
    use kaka::model::{PhotoListItem, Status};

    let mk = |name: &str, status: Status, paired: bool| PhotoListItem {
        id: 1,
        original_filename: name.to_string(),
        current_path: format!("C:/p/{name}"),
        folder_path: "C:/p".to_string(),
        status,
        capture_time: "2026-01-01 00:00:00".to_string(),
        file_size: 10,
        thumb_hash: None,
        camera_model: None,
        lens_model: None,
        iso: None,
        aperture: None,
        shutter_speed: None,
        focal_length: None,
        decode_failed: false,
        preview_only: false,
        pair_group_id: if paired { Some(7) } else { None },
        rotation_override: 0,
    };
    let del = mk("DSC_0001.NEF", Status::Delete, true);
    let rev = mk("IMG_0002.JPG", Status::Reviewed, false);

    // @关键词（中文与英文均可用）。
    assert!(eval_search(&del, "@待删"));
    assert!(!eval_search(&del, "@已阅"));
    assert!(eval_search(&rev, "@reviewed"));
    // 或：||
    assert!(eval_search(&del, "@待删||@已阅"));
    assert!(eval_search(&rev, "@待删||@已阅"));
    assert!(!eval_search(&rev, "@待删||@未处理"));
    // 与：&& 与空白（隐式与）
    assert!(eval_search(&del, "@待删 && DSC_0001"));
    assert!(!eval_search(&del, "@待删 && IMG"));
    assert!(eval_search(&del, "DSC 0001"), "whitespace = implicit AND");
    assert!(!eval_search(&del, "DSC IMG"), "both substrings must match");
    // 非：!
    assert!(eval_search(&rev, "!@待删"));
    assert!(!eval_search(&del, "!@待删"));
    // 配对 / 丢失
    assert!(eval_search(&del, "@配对"));
    assert!(!eval_search(&rev, "@配对"));
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
fn cache_index_reconcile_and_cleanup() {
    use kaka::io::cache_clean::{reconcile, run_cleanup};
    use kaka::io::cache_index::CacheIndex;

    let root = temp_root();
    let dir = root.join("cache");
    std::fs::create_dir_all(dir.join("thumbs")).unwrap();
    std::fs::create_dir_all(dir.join("previews")).unwrap();
    std::fs::write(dir.join("thumbs/a.jpg"), vec![0u8; 10]).unwrap();
    std::fs::write(dir.join("thumbs/b.jpg"), vec![0u8; 20]).unwrap();
    std::fs::write(dir.join("previews/c.jpg"), vec![0u8; 30]).unwrap();

    let idx = CacheIndex::open(&root.join("cache_index.db")).unwrap();
    // Register all three via reconcile (as if written before the index existed).
    assert_eq!(reconcile(&idx, &dir), 3);
    assert_eq!(reconcile(&idx, &dir), 0, "second reconcile is a no-op");
    assert_eq!(idx.count().unwrap(), 3);
    assert_eq!(idx.total_size().unwrap(), 60);

    // Backdate: a created 40 days ago (expired at 30 days), c last accessed
    // 10 days ago (LRU-oldest), b freshly accessed.
    let now = chrono::Local::now();
    let fmt = |d: chrono::Duration| (now - d).format("%Y-%m-%d %H:%M:%S").to_string();
    idx.record_write_at(
        "thumbs/a.jpg",
        "thumb",
        10,
        &fmt(chrono::Duration::days(40)),
        &fmt(chrono::Duration::days(40)),
    )
    .unwrap();
    idx.record_write_at(
        "thumbs/b.jpg",
        "thumb",
        20,
        &fmt(chrono::Duration::days(1)),
        &fmt(chrono::Duration::days(0)),
    )
    .unwrap();
    idx.record_write_at(
        "previews/c.jpg",
        "preview",
        30,
        &fmt(chrono::Duration::days(2)),
        &fmt(chrono::Duration::days(10)),
    )
    .unwrap();

    // 1) Expire pass: a.jpg removed from disk + index.
    let mut prog = |_d: usize| -> bool { true };
    let cap = 1024 * 1024 * 1024; // 1 GB — no capacity eviction expected
    let stats = run_cleanup(&idx, &dir, cap, 30, usize::MAX, &mut prog).unwrap();
    assert_eq!(stats.expired, 1);
    assert_eq!(stats.deleted, 1);
    assert!(!dir.join("thumbs/a.jpg").exists());
    assert_eq!(idx.total_size().unwrap(), 50);

    // 2) Capacity pass: cap 25 bytes, total 50 > cap → evict LRU-oldest (c)
    //    until usage drops below the 85% floor (21 bytes).
    let stats2 = run_cleanup(&idx, &dir, 25, 365, usize::MAX, &mut prog).unwrap();
    assert_eq!(stats2.deleted, 1);
    assert!(!dir.join("previews/c.jpg").exists());
    assert!(dir.join("thumbs/b.jpg").exists(), "most recently accessed entry survives");
    assert_eq!(idx.total_size().unwrap(), 20);
}

#[test]
fn decode_failed_flag_roundtrip() {
    use kaka::model::Status;

    let root = temp_root();
    let db_path = root.join("df.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("photos");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [5, 10, 15]);
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();

    let items =
        db::photos::list_items_in_folder(&db, &src.to_string_lossy(), kaka::model::SortOrder::FilenameAsc).unwrap();
    let id = items[0].id;
    assert!(!items[0].decode_failed);

    // Persist the failure (PRD 7.4.3), verify it reads back, then clear it
    // (the 强制重试 path).
    db::photos::set_decode_failed(&db, id, true).unwrap();
    let p = db::photos::get_photo(&db, id).unwrap().unwrap();
    assert!(p.decode_failed);
    db::photos::set_decode_failed(&db, id, false).unwrap();
    let p = db::photos::get_photo(&db, id).unwrap().unwrap();
    assert!(!p.decode_failed);
    // Status must be untouched by the flag updates.
    assert_eq!(p.status, Status::Untreated);
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
    let outcome = import::add_mode_import(&mut db, std::path::Path::new(&bordered), true, true, &mut prog, None).unwrap();
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

#[test]
fn rotation_override_roundtrip_and_list_mapping() {
    // PRD 7.2: R / Ctrl+R / Shift+R persist rotation_override (0 = EXIF,
    // 1..=3 = extra 90/180/270° CW) in the DB and the workspace item mirror.
    use kaka::app::state::AppState;
    use kaka::model::{AppConfig, Status};

    let root = temp_root();
    let db_path = root.join("rot.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("photos");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [1, 2, 3]);
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();

    let mut app = AppState::new(db, AppConfig::default());
    app.open_workspace(&src.to_string_lossy(), kaka::model::SortOrder::FilenameAsc)
        .unwrap();
    assert_eq!(app.ws.items.len(), 1);
    assert_eq!(app.ws.items[0].rotation_override, 0);

    // R → 1, R → 2, Ctrl+R (−1) → 1 → 0; CCW past EXIF wraps to 270°.
    assert_eq!(app.rotate_current(1).unwrap(), Some(1));
    assert_eq!(app.rotate_current(1).unwrap(), Some(2));
    assert_eq!(app.rotate_current(-1).unwrap(), Some(1));
    assert_eq!(app.rotate_current(-1).unwrap(), Some(0));
    assert_eq!(app.rotate_current(-1).unwrap(), Some(3), "CCW from EXIF wraps to 270°");
    assert_eq!(app.rotate_current(0).unwrap(), Some(0), "Shift+R resets to EXIF");

    // The in-memory mirror follows, and the value persists across connections.
    app.rotate_current(2).unwrap();
    assert_eq!(app.ws.items[0].rotation_override, 2);
    app.rotate_current(1).unwrap(); // 2 -> 3
    let db2 = Db::open(&db_path).unwrap();
    let items = db::photos::list_items_in_folder(
        &db2,
        &src.to_string_lossy(),
        kaka::model::SortOrder::FilenameAsc,
    )
    .unwrap();
    assert_eq!(items[0].rotation_override, 3);
    // Rotation must not touch the status (no undo-stack entry either).
    assert_eq!(items[0].status, Status::Untreated);
}

#[test]
fn step_wrap_at_end_toggle() {
    use kaka::app::state::AppState;
    use kaka::model::AppConfig;

    let root = temp_root();
    let db_path = root.join("wrap.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("photos");
    std::fs::create_dir_all(&src).unwrap();
    for i in 0..3 {
        make_jpeg(&src.join(format!("DSC_{i:04}.JPG")), [i as u8, 30, 60]);
    }
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();

    let mut app = AppState::new(db, AppConfig::default());
    app.open_workspace(&src.to_string_lossy(), kaka::model::SortOrder::FilenameAsc).unwrap();
    assert_eq!(app.ws.items.len(), 3);

    // No wrap: blocked at both ends, index unchanged.
    app.ws.current_index = 0;
    assert!(app.step(-1, false), "at first without wrap must be blocked");
    assert_eq!(app.ws.current_index, 0);
    app.ws.current_index = 2;
    assert!(app.step(1, false), "at last without wrap must be blocked");
    assert_eq!(app.ws.current_index, 2);

    // Wrap: last -> first on forward, first -> last on backward.
    assert!(!app.step(1, true));
    assert_eq!(app.ws.current_index, 0, "wrapped from last to first");
    assert!(!app.step(-1, true));
    assert_eq!(app.ws.current_index, 2, "wrapped from first to last");
    // Normal in-range moves are unaffected by the flag.
    assert!(!app.step(-1, true));
    assert_eq!(app.ws.current_index, 1);
}

#[test]
fn cache_migration_copies_tree() {
    use kaka::io::cache_clean::migrate_cache;
    use kaka::io::cache_index::CacheIndex;
    use std::path::Path;

    let root = temp_root();
    let old = root.join("old_cache");
    let new = root.join("new_cache");
    std::fs::create_dir_all(old.join("thumbs")).unwrap();
    std::fs::create_dir_all(old.join("previews")).unwrap();
    std::fs::write(old.join("thumbs/a.jpg"), vec![1u8; 16]).unwrap();
    std::fs::write(old.join("previews/b.jpg"), vec![2u8; 32]).unwrap();

    // A real index db so the checkpoint path is exercised.
    let idx = CacheIndex::open(&old.join("cache_index.db")).unwrap();
    idx.record_write("thumbs/a.jpg", "thumb", 16).unwrap();
    drop(idx); // release the file so the old tree can be deleted later

    let n = migrate_cache(&old, &new).unwrap();
    // 3 essential files (+ SQLite -wal/-shm sidecars when present).
    assert!(n >= 3, "expected at least 3 copied files, got {n}");
    assert!(new.join("thumbs/a.jpg").exists());
    assert!(new.join("previews/b.jpg").exists());
    assert!(new.join("cache_index.db").exists());

    // The copied index still resolves its entries (rel-path based).
    let new_idx = CacheIndex::open(&new.join("cache_index.db")).unwrap();
    assert!(new_idx.has("thumbs/a.jpg").unwrap());

    // Same-path migration is rejected.
    assert!(migrate_cache(&old, &old).is_err());

    // Caller-side cleanup after reset (mirrors app::start_cache_migration).
    kaka::io::cache_index::reset_global();
    std::fs::remove_dir_all(&old).unwrap();
    assert!(!Path::new(&old).exists());
}

#[test]
fn cache_rebuild_regenerates_missing_and_counts_failures() {
    let root = temp_root();
    let db_path = root.join("rebuild.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("import");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [10, 20, 30]);
    make_jpeg(&src.join("DSC_0002.JPG"), [40, 50, 60]);

    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();
    let items = db::photos::list_items_in_folder(
        &db,
        &src.to_string_lossy(),
        kaka::model::SortOrder::CaptureTimeAsc,
    )
    .unwrap();
    assert_eq!(items.len(), 2);
    let hash = |p: &kaka::model::PhotoListItem| p.thumb_hash.clone().unwrap();

    // Caches exist after generation, then wipe them to simulate a broken cache.
    for p in &items {
        assert!(thumbnails::generate_caches(
            Path::new(&p.current_path),
            p.thumb_hash.as_ref().unwrap(),
            1.0
        )
        .unwrap());
    }
    for p in &items {
        std::fs::remove_file(thumbnails::thumb_path(&hash(p), 1.0)).unwrap();
        std::fs::remove_file(thumbnails::preview_path(&hash(p))).unwrap();
    }

    // Rebuild regenerates everything and reports progress per photo.
    let mut progress: Vec<(usize, usize)> = Vec::new();
    let (done, failed) = kaka::app::cache_rebuild::rebuild_all_caches(&db, 1.0, || false, |d, t| {
        progress.push((d, t));
    });
    assert_eq!(done, 2);
    assert_eq!(failed, 0);
    assert_eq!(progress.last(), Some(&(2, 2)));
    for p in &items {
        assert!(thumbnails::thumb_path(&hash(p), 1.0).exists());
        assert!(thumbnails::preview_path(&hash(p)).exists());
    }

    // A photo whose source file vanished counts as a failure, not a crash.
    std::fs::remove_file(&items[0].current_path).unwrap();
    std::fs::remove_file(thumbnails::thumb_path(&hash(&items[0]), 1.0)).unwrap();
    let (done, failed) =
        kaka::app::cache_rebuild::rebuild_all_caches(&db, 1.0, || false, |_, _| {});
    assert_eq!(done, 2);
    assert_eq!(failed, 1);

    // Cancellation before the first photo regenerates nothing (done photos stay).
    std::fs::remove_file(thumbnails::thumb_path(&hash(&items[1]), 1.0)).unwrap();
    std::fs::remove_file(thumbnails::preview_path(&hash(&items[1]))).unwrap();
    let (done, _failed) = kaka::app::cache_rebuild::rebuild_all_caches(&db, 1.0, || true, |_, _| {});
    assert_eq!(done, 0);
    assert!(!thumbnails::thumb_path(&hash(&items[1]), 1.0).exists());
    assert!(!thumbnails::preview_path(&hash(&items[1])).exists());
}

#[test]
fn write_xmp_sidecars_merges_existing_sidecar() {
    use kaka::app::export::write_xmp_sidecars;

    let root = temp_root();
    let db_path = root.join("xmp_merge.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("import");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [10, 20, 30]);
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();

    // A pre-existing sidecar written by another tool, with custom fields.
    let sidecar = src.join("DSC_0001.xmp");
    std::fs::write(
        &sidecar,
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/">
   <dc:description><rdf:Alt><rdf:li xml:lang="x-default">Custom caption</rdf:li></rdf:Alt></dc:description>
   <photoshop:City>Qingdao</photoshop:City>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#,
    )
    .unwrap();

    let n = write_xmp_sidecars(&db, &src.to_string_lossy(), 4).unwrap();
    assert_eq!(n, 1);

    let out = std::fs::read_to_string(&sidecar).unwrap();
    // Original fields preserved…
    assert!(out.contains("Custom caption"), "custom caption lost:\n{out}");
    assert!(out.contains("<photoshop:City>Qingdao</photoshop:City>"));
    // …and the keep-mark fields written.
    assert!(out.contains("<xmp:Rating>4</xmp:Rating>"));
    assert!(out.contains("<xmp:Label>Kaka:Keep</xmp:Label>"));
}

#[test]
fn import_prescan_marks_new_exists_and_repair() {
    use kaka::app::import::{prescan_mark, PrescanMark};

    let root = temp_root();
    let db_path = root.join("prescan.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    // dirA holds the first import; dirB will hold a moved copy of one file.
    let dir_a = root.join("a");
    let dir_b = root.join("b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    make_jpeg(&dir_a.join("DSC_0001.JPG"), [10, 20, 30]); // will move to dirB
    make_jpeg(&dir_a.join("DSC_0002.JPG"), [40, 50, 60]); // stays → Exists

    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &dir_a, true, true, &mut prog, None).unwrap();

    // Move DSC_0001 to dirB (rename keeps size + mtime → same three elements)
    // and delete the original so the library path becomes invalid.
    std::fs::rename(dir_a.join("DSC_0001.JPG"), dir_b.join("DSC_0001.JPG")).unwrap();
    // A genuinely new file the library has never seen.
    make_jpeg(&dir_a.join("DSC_0003.JPG"), [70, 80, 90]);

    let mut cancelled_checks = 0usize;
    let items = prescan_mark(&mut db, &root, true, 5, &mut |d, t| {
        if d >= 2 {
            cancelled_checks += 1;
            return false; // exercise the cancel path once past the first files
        }
        true
    })
    .unwrap();
    assert!(items.is_none(), "cancel should yield None");
    let _ = cancelled_checks;

    // Full scan (no cancel).
    let items = prescan_mark(&mut db, &root, true, 5, &mut |_d, _t| true)
        .unwrap()
        .unwrap();
    let mark_of = |name: &str| {
        items
            .iter()
            .find(|i| i.filename == name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .mark
    };
    assert_eq!(mark_of("DSC_0002.JPG"), PrescanMark::Exists, "in library + valid path");
    assert_eq!(
        mark_of("DSC_0001.JPG"),
        PrescanMark::PathRepair,
        "three-element match but library path is gone"
    );
    assert_eq!(mark_of("DSC_0003.JPG"), PrescanMark::New);
    // Repair items point at the new location (dirB), not the stale one.
    let repair = items.iter().find(|i| i.mark == PrescanMark::PathRepair).unwrap();
    assert!(repair.path.ends_with("DSC_0001.JPG"));
    assert_ne!(repair.path, dir_a.join("DSC_0001.JPG").to_string_lossy());

    // Non-recursive scan of dirB sees only the moved file.
    let items_b = prescan_mark(&mut db, &dir_b, false, 5, &mut |_d, _t| true)
        .unwrap()
        .unwrap();
    assert_eq!(items_b.len(), 1);
    assert_eq!(items_b[0].mark, PrescanMark::PathRepair);
}

#[test]
fn import_report_csv_and_structured_repairs() {
    use kaka::app::import::{export_failure_csv, export_repair_csv, ImportFailure, PathRepair};

    let root = temp_root();

    // CSV writers: BOM + CRLF + header + quoting of comma/quote fields.
    let failures = vec![ImportFailure {
        source_path: r#"a,DSC_1.JPG"#.into(),
        target_path: r#"out"DSC_1.JPG"#.into(),
        reason_code: "COPY_FAILED".into(),
        reason: "磁盘已满".into(),
        file_size: 123,
        capture_time: "2026-01-01 10:00:00".into(),
    }];
    let csv = root.join("failures.csv");
    let n = export_failure_csv(csv.to_string_lossy().as_ref(), &failures).unwrap();
    assert_eq!(n, 1);
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(text.starts_with('\u{feff}'));
    assert!(text.contains("\r\n"));
    assert!(text.contains("序号,源路径,目标路径,失败原因代码,失败原因描述,文件大小,三要素捕获时间"));
    assert!(text.contains(r#""a,DSC_1.JPG""#), "comma field must be quoted: {text}");

    let repairs = vec![PathRepair {
        old_path: "old".into(),
        new_path: "new".into(),
        original_filename: "DSC_0001.JPG".into(),
        capture_time: "2026-01-01 10:00:00".into(),
        repaired_at: "2026-09-06 19:00:00".into(),
    }];
    let csv2 = root.join("repairs.csv");
    export_repair_csv(csv2.to_string_lossy().as_ref(), &repairs).unwrap();
    let text2 = std::fs::read_to_string(&csv2).unwrap();
    assert!(text2.contains("序号,旧路径,新路径,原始文件名,三要素捕获时间,修复时间戳"));
    assert!(text2.contains("old,new,DSC_0001.JPG"));

    // End-to-end: re-importing a moved file produces one structured repair
    // with the old/new paths filled in.
    let db_path = root.join("repair.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();
    let dir_a = root.join("ra");
    let dir_b = root.join("rb");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    make_jpeg(&dir_a.join("DSC_0001.JPG"), [10, 20, 30]);
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &dir_a, true, true, &mut prog, None).unwrap();
    std::fs::rename(dir_a.join("DSC_0001.JPG"), dir_b.join("DSC_0001.JPG")).unwrap();

    let outcome = import::add_mode_import(&mut db, &root, true, true, &mut prog, None).unwrap();
    assert_eq!(outcome.path_repaired, 1);
    assert_eq!(outcome.added, 0);
    let r = &outcome.repairs[0];
    assert!(r.old_path.ends_with("DSC_0001.JPG"));
    assert!(r.new_path.ends_with("DSC_0001.JPG"));
    assert_ne!(r.old_path, r.new_path);
    assert_eq!(r.original_filename, "DSC_0001.JPG");
    assert!(!r.repaired_at.is_empty());
}

#[test]
fn remove_missing_record_updates_rows_and_cache_registry() {
    let root = temp_root();
    let db_path = root.join("missing.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let src = root.join("import");
    std::fs::create_dir_all(&src).unwrap();
    make_jpeg(&src.join("DSC_0001.JPG"), [10, 20, 30]);
    make_jpeg(&src.join("DSC_0002.JPG"), [40, 50, 60]);
    let mut prog = |_p: &str, _d: usize, _t: usize, _n: &str| -> bool { true };
    import::add_mode_import(&mut db, &src, true, true, &mut prog, None).unwrap();
    let items = db::photos::list_items_in_folder(
        &db,
        &src.to_string_lossy(),
        kaka::model::SortOrder::FilenameAsc,
    )
    .unwrap();
    assert_eq!(items.len(), 2);
    let victim = &items[0];
    let hash = victim.thumb_hash.clone().unwrap();

    // Generate + register the cache files for the victim's hash.
    assert!(thumbnails::generate_caches(
        Path::new(&victim.current_path),
        &hash,
        1.0
    )
    .unwrap());
    let global_idx = kaka::io::cache_index::CacheIndex::open_default().unwrap();
    assert!(
        global_idx
            .has(format!("thumbs/{hash}.jpg").as_str())
            .unwrap(),
        "registration should exist before cleanup"
    );
    drop(global_idx);

    // Delete the library row…
    db::photos::delete_photo(&db, victim.id).unwrap();
    let counts = db::photos::status_counts(&db, "").unwrap();
    assert_eq!(counts.total, 1, "photos row count must drop 2 → 1");

    // …and clean the disk-cache registration (files stay on disk).
    kaka::io::cache_index::delete_hash_registrations(&hash);
    let global_idx = kaka::io::cache_index::CacheIndex::open_default().unwrap();
    assert!(
        !global_idx
            .has(format!("thumbs/{hash}.jpg").as_str())
            .unwrap(),
        "registration must be cleaned"
    );
    assert!(
        thumbnails::thumb_path(&hash, 1.0).exists(),
        "the cache FILE itself is kept"
    );
    drop(global_idx);
    kaka::io::cache_index::reset_global();
}

#[test]
fn workspace_remove_item_navigation() {
    use kaka::app::state::Workspace;
    use kaka::model::{PhotoListItem, Status};

    let item = |id: i64| PhotoListItem {
        id,
        original_filename: format!("DSC_{id:04}.JPG"),
        current_path: format!("x/DSC_{id:04}.JPG"),
        folder_path: "x".into(),
        status: Status::Untreated,
        capture_time: String::new(),
        file_size: 1,
        thumb_hash: None,
        camera_model: None,
        lens_model: None,
        iso: None,
        aperture: None,
        shutter_speed: None,
        focal_length: None,
        decode_failed: false,
        preview_only: false,
        pair_group_id: None,
        rotation_override: 0,
    };

    let mut ws = Workspace::empty();
    ws.items = vec![item(1), item(2), item(3)];
    ws.current_index = 1; // showing item 2

    // Removing the current shows the NEXT photo at the same index.
    ws.remove_item(2);
    assert_eq!(ws.items.len(), 2);
    assert_eq!(ws.items[ws.current_index].id, 3, "next photo should show");
    assert!(!ws.selection.contains(&2));

    // Removing the last shown photo steps back one.
    ws.remove_item(3);
    assert_eq!(ws.items.len(), 1);
    assert_eq!(ws.items[ws.current_index].id, 1);

    // Removing everything empties the workspace safely.
    ws.remove_item(1);
    assert!(ws.items.is_empty());
    assert_eq!(ws.current_index, 0);
}

#[test]
fn schema_v1_migrates_to_v2_with_backfill() {
    let root = temp_root();
    let db_path = root.join("kaka.db");
    let mut db = Db::open(&db_path).unwrap();

    // Hand-build a version-1 database (photos without the numeric columns).
    db.conn
        .execute_batch(
            r#"
            CREATE TABLE meta (
                id              INTEGER PRIMARY KEY CHECK (id = 1),
                schema_version  INTEGER NOT NULL DEFAULT 1,
                app_version     TEXT,
                created_at      TEXT DEFAULT (datetime('now')),
                last_migrated_at TEXT
            );
            CREATE TABLE photos (
                id                    INTEGER PRIMARY KEY AUTOINCREMENT,
                original_filename     TEXT NOT NULL,
                file_size             INTEGER NOT NULL,
                capture_time          TEXT NOT NULL,
                current_path          TEXT NOT NULL,
                folder_path           TEXT NOT NULL,
                status                INTEGER DEFAULT 0,
                thumb_hash            TEXT,
                decode_failed         INTEGER DEFAULT 0,
                preview_only          INTEGER DEFAULT 0,
                rotation_override     INTEGER DEFAULT 0,
                exif_orientation      INTEGER DEFAULT 1,
                pair_group_id         INTEGER,
                iso                   INTEGER,
                aperture              TEXT,
                shutter_speed         TEXT,
                focal_length          INTEGER,
                camera_model          TEXT,
                lens_model            TEXT,
                capture_time_source   TEXT DEFAULT 'exif_original',
                import_time           TEXT DEFAULT (datetime('now')),
                last_access_time      TEXT DEFAULT (datetime('now')),
                marked_delete_time    TEXT,
                marked_review_time    TEXT
            );
            INSERT INTO meta (id, schema_version, app_version) VALUES (1, 1, 'test');
            INSERT INTO photos (original_filename, file_size, capture_time, current_path,
                                folder_path, aperture, shutter_speed)
                VALUES ('A.JPG', 1, '2026-01-01 10:00:00', 'x/A.JPG', 'x', 'f/5.6', '1/200s');
            INSERT INTO photos (original_filename, file_size, capture_time, current_path,
                                folder_path, aperture, shutter_speed)
                VALUES ('B.JPG', 2, '2026-01-01 10:00:01', 'x/B.JPG', 'x', 'f/8', '30s');
            INSERT INTO photos (original_filename, file_size, capture_time, current_path,
                                folder_path)
                VALUES ('C.JPG', 3, '2026-01-01 10:00:02', 'x/C.JPG', 'x');
        "#,
    )
    .unwrap();

    kaka::db::schema::migrate(&mut db).unwrap();

    // Version bumped, backup created, columns exist with backfilled values.
    assert_eq!(db.schema_version().unwrap(), 2);
    assert!(db_path.with_file_name("kaka.db.v1.bak").exists());
    let rows: Vec<(Option<f64>, Option<f64>)> = db
        .conn
        .prepare("SELECT aperture_num, shutter_num FROM photos ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let (a, s) = rows[0];
    assert!((a.unwrap() - 5.6).abs() < 1e-9);
    assert!((s.unwrap() - 0.005).abs() < 1e-9);
    let (a, s) = rows[1];
    assert_eq!(a.unwrap(), 8.0);
    assert_eq!(s.unwrap(), 30.0);
    assert!(rows[2].0.is_none() && rows[2].1.is_none(), "no strings → NULL");

    // The numeric range predicate works on the backfilled data.
    let n: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM photos WHERE aperture_num >= 4 AND aperture_num <= 8",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 2);
}

#[test]
fn filter_aperture_shutter_ranges() {
    use kaka::model::{Filter, Photo, SortOrder};

    let root = temp_root();
    let db_path = root.join("range.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let folder = root.join("photos").to_string_lossy().into_owned();
    let mk = |name: &str, f: f64, s: f64| Photo {
        id: 0,
        original_filename: name.into(),
        file_size: 1,
        capture_time: "2026-01-01 10:00:00".into(),
        current_path: format!("{folder}/{name}"),
        folder_path: folder.clone(),
        status: kaka::model::Status::Untreated,
        thumb_hash: None,
        decode_failed: false,
        preview_only: false,
        rotation_override: 0,
        exif_orientation: 1,
        pair_group_id: None,
        iso: None,
        aperture: Some(format!("f/{}", f)),
        shutter_speed: Some("1/500s".into()),
        aperture_num: Some(f),
        shutter_num: Some(s),
        focal_length: None,
        camera_model: None,
        lens_model: None,
        capture_time_source: "exif_original".into(),
        import_time: String::new(),
        last_access_time: String::new(),
        marked_delete_time: None,
        marked_review_time: None,
    };
    db::photos::insert_photo(&db, &mk("A.JPG", 5.6, 0.002)).unwrap();
    db::photos::insert_photo(&db, &mk("B.JPG", 11.0, 0.016667)).unwrap();
    db::photos::insert_photo(&db, &mk("C.JPG", 1.8, 30.0)).unwrap();
    // A photo without numeric values: range filters must exclude it.
    let mut no_num = mk("D.JPG", 0.0, 0.0);
    no_num.aperture = None;
    no_num.shutter_speed = None;
    no_num.aperture_num = None;
    no_num.shutter_num = None;
    db::photos::insert_photo(&db, &no_num).unwrap();

    let ids = |filter: Filter| -> Vec<String> {
        db::photos::list_items_filtered(&db, &folder, SortOrder::FilenameAsc, &filter)
            .unwrap()
            .into_iter()
            .map(|p| p.original_filename)
            .collect()
    };

    // Aperture range 4..=8 → only A.
    let out = ids(Filter {
        aperture_min: Some(4.0),
        aperture_max: Some(8.0),
        ..Default::default()
    });
    assert_eq!(out, vec!["A.JPG".to_string()]);

    // Fastest shutter ≤ 1/125 (0.008s) → only A.
    let out = ids(Filter {
        shutter_max: Some(0.008),
        ..Default::default()
    });
    assert_eq!(out, vec!["A.JPG".to_string()]);

    // Slowest shutter ≥ 1s → only C.
    let out = ids(Filter {
        shutter_min: Some(1.0),
        ..Default::default()
    });
    assert_eq!(out, vec!["C.JPG".to_string()]);

    // Combined: aperture 4..=8 AND shutter ≥ 1s → impossible → empty.
    let out = ids(Filter {
        aperture_min: Some(4.0),
        aperture_max: Some(8.0),
        shutter_min: Some(1.0),
        ..Default::default()
    });
    assert!(out.is_empty());

    // No filter → everything (including the NULL-numeric photo).
    let out = ids(Filter::default());
    assert_eq!(out.len(), 4);

}

#[cfg(feature = "gui")]
#[test]
fn preload_worker_decodes_cached_preview_and_skips_missing() {
    use kaka::app::preload::{PreloadJob, PreloadWorker};
    use kaka::io::thumbnails;

    let root = temp_root();
    let src = root.join("DSC_0001.JPG");
    make_jpeg(&src, [120, 60, 200]);
    let hash = thumbnails::thumb_hash_for(src.to_string_lossy().as_ref(), 1, "");

    // Generate the 1920px disk preview the worker is expected to decode.
    assert!(thumbnails::generate_preview(
        Path::new(&src),
        &thumbnails::preview_path(&hash),
        thumbnails::PREVIEW_LONG_EDGE,
        thumbnails::PREVIEW_QUALITY
    )
    .unwrap());

    let mut w = PreloadWorker::new();
    w.set_queue(vec![PreloadJob {
        photo_id: -1,
        hash: hash.clone(),
        path: src.to_string_lossy().into_owned(),
    }]);

    let mut got = None;
    for _ in 0..300 {
        for d in w.poll() {
            got = Some(d);
        }
        if got.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let done = got.expect("preload should complete");
    assert_eq!(done.photo_id, -1);
    assert_eq!(done.hash, hash);
    let img = done.image.expect("cached preview should decode");
    assert!(img.width() > 0 && (img.width() as u32) <= thumbnails::PREVIEW_LONG_EDGE);

    // A missing disk preview yields Done { image: None } (caller skips it).
    w.set_queue(vec![PreloadJob {
        photo_id: -2,
        hash: "nonexistent".into(),
        path: String::new(),
    }]);
    let mut got2 = None;
    for _ in 0..300 {
        for d in w.poll() {
            got2 = Some(d);
        }
        if got2.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let done2 = got2.expect("missing-preview job should still report");
    assert_eq!(done2.photo_id, -2);
    assert!(done2.image.is_none());
}

// ---- P2-10 拖入图片文件定位：find_by_path 查询（UI 3.3） ----

#[test]
fn find_by_path_matches_photo_case_insensitively() {
    let root = temp_root();
    let db_path = root.join("locate.db");
    let mut db = Db::open(&db_path).unwrap();
    db::schema::init(&mut db).unwrap();
    db::schema::migrate(&mut db).unwrap();

    let file = root.join("DSC_0001.jpg");
    make_jpeg(&file, [200, 30, 30]);
    let p = kaka::model::Photo {
        id: 0,
        original_filename: "DSC_0001.jpg".into(),
        file_size: 1234,
        capture_time: "2026-01-01 10:00:00".into(),
        current_path: file.to_string_lossy().into_owned(),
        folder_path: root.to_string_lossy().into_owned(),
        status: kaka::model::Status::Untreated,
        thumb_hash: None,
        decode_failed: false,
        preview_only: false,
        rotation_override: 0,
        exif_orientation: 1,
        pair_group_id: None,
        iso: None,
        aperture: None,
        shutter_speed: None,
        aperture_num: None,
        shutter_num: None,
        focal_length: None,
        camera_model: None,
        lens_model: None,
        capture_time_source: "exif_original".into(),
        import_time: "2026-01-01 10:00:00".into(),
        last_access_time: "2026-01-01 10:00:00".into(),
        marked_delete_time: None,
        marked_review_time: None,
    };
    let id = db::photos::insert_photo(&db, &p).unwrap().expect("insert");
    assert_eq!(id, 1);

    // 精确匹配命中。
    let hit = db::photos::find_by_path(&db, &file.to_string_lossy()).unwrap().expect("found");
    assert_eq!(hit.id, id);
    assert_eq!(hit.folder_path, root.to_string_lossy());

    // 大小写不同的路径也能命中（Windows 路径大小写不敏感）。
    let upper = file.to_string_lossy().to_uppercase();
    let hit2 = db::photos::find_by_path(&db, &upper).unwrap().expect("case-insensitive hit");
    assert_eq!(hit2.id, id);

    // 不存在的路径返回 None。
    assert!(db::photos::find_by_path(&db, "C:/nope/missing.jpg").unwrap().is_none());
}

// ---- P2-8 首次启动引导：onboarding_done 配置持久化（PRD 十六） ----

#[test]
fn onboarding_done_flag_roundtrips_through_toml() {
    let mut cfg = kaka::model::AppConfig::default();
    assert!(!cfg.onboarding_done, "default must not be done");
    cfg.onboarding_done = true;
    cfg.default_target_dir = "D:/photos".into();

    let text = toml::to_string(&cfg).unwrap();
    let back: kaka::model::AppConfig = toml::from_str(&text).unwrap();
    assert!(back.onboarding_done);
    assert_eq!(back.default_target_dir, "D:/photos");

    // 旧版 config.toml（无该字段）解析回默认 false，引导仍会弹出。
    let old: kaka::model::AppConfig = toml::from_str("language = \"en\"\npair_time_threshold_secs = 7\n").unwrap();
    assert!(!old.onboarding_done);
    assert_eq!(old.language, "en");
    assert_eq!(old.pair_time_threshold_secs, 7);
}
