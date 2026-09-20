# Kaka

English ｜ [简体中文](README.zh-CN.md)

## Overview

Kaka is a Windows desktop app for importing and quickly culling camera photos. It does exactly two things: build a local database index of your photos (copy mode for memory cards, add mode for existing folders), and let you blast through them with keyboard-driven culling — Q to mark for deletion, E to skip, then send everything marked to the recycle bin. Post-processing is left to Lightroom.

Built with Rust and egui as a single static executable with SQLite compiled in. No runtime, no external DLLs — just run the exe. Photo files are only ever read or moved to the recycle bin; their contents are never modified.

## Features

Import:

- Two modes: copy (physical copy from a card, original filenames preserved) and add (index files in place)
- Recursive subfolder scanning; target directory layouts: keep original structure / group by capture date / flat (name collisions get a `_dup` suffix, matching XMP sidecars renamed in sync)
- Pre-import scan: dedup marking (new / already exists / path repairable) with a checkable file grid
- Free disk space pre-check (blocks when insufficient), resumable imports, optional card cleanup after a fully successful import
- Auto-open import window when a removable drive is detected; dropping a folder onto the window starts an add-mode import
- Import report (succeeded / failed / repaired), failure list exportable as CSV

RAW+JPG pairing:

- Auto-paired when same folder, same basename, and EXIF capture-time delta within the threshold (default 5 s, adjustable 1–30)
- Pairs shown as a single photo in the culling view (RAW shown, R+J badge)
- Q/E/U applies to the whole pair, delete box deletes/restores as a unit, export copies both files; if one file goes missing the pair is unlinked on next launch

Culling:

- Keyboard-driven: Q mark delete / E mark reviewed / U reset; arrows, A/D, Space to navigate
- Z-key 100% zoom: full-frame background RAW decode, histogram-based tone matching (RAW render stays consistent with the preview), viewport minimap, Ctrl+wheel free zoom, Ctrl+drag pan
- Rotate: R clockwise / Ctrl+R counter-clockwise / Shift+R reset to EXIF orientation
- Search: 300 ms debounce, Enter applies immediately; `@delete / @reviewed / @untreated / @missing / @paired` prefixes, `&& / || / !` logic, `@` autocomplete
- Advanced filter: status, camera, lens, ISO, focal length, aperture, shutter speed, date range, format, missing, paired
- Multi-select (Ctrl/Shift click, Ctrl+A) and batch marking (Ctrl+Q/E/U with confirmation)
- Undo/redo (Ctrl+Z / Ctrl+Y, single-key marks only, up to 100 entries)
- Delete box: thumbnail grid, pairs collapsed into delete units, restore or send everything to the recycle bin as a unit
- Histogram (RGB / single-channel), highlight/shadow clipping warnings, numeric photo jumping, filter-complete toast
- Dropping photo files onto the window: if they share one folder, switches to that workspace and locates the first photo

Export:

- Copy kept photos (three layouts, optional original XMP sidecars and rotation metadata, runs in background)
- Kept-photo list (.txt / .csv)
- Write XMP marks (Kaka:Keep + rating; sidecar for RAW, embedded for JPEG/PNG)
- Send to Lightroom Classic (temporary .lrtemplate collection; a 15-second launch timeout shows a toast with an "Open folder" button; the entry is greyed out when LR is not installed)

Data safety and maintenance:

- Corrupted database: three choices — auto-repair (prefers manual backups) / pick a backup manually / discard and start fresh
- Settings panel: integrity check, manual backup (keeps 5), restore from backup
- Crash recovery prompt after an abnormal shutdown; interrupted imports can be resumed
- Deletion only goes to the recycle bin, never a hard delete

UI and settings:

- First-run three-step wizard (welcome / basic settings / choose import, skippable, shown once)
- Top-bar path dropdown (10 recent folders, browse, copy path), Ctrl+L to type a path directly
- 12 core actions remappable (conflict detection, restore defaults, applies on save)
- Chinese/English UI, F11 borderless fullscreen, minimum window 1024×640, Per-Monitor V2 DPI
- Caching: two-tier disk cache (thumbnails + previews), background pre-decoding, 2 GB in-memory LRU for full-res textures, cache path customizable with migration
- Logs: `%APPDATA%/Kaka/logs/`, plain text, kept for 14 days

Supported formats:

- RAW: NEF/NRW, CR2/CR3, ARW/SR2, RAF, PEF/PTX, ORF, RW2, DNG, IIQ, 3FR, X3F, etc.
- JPEG, PNG, TIFF
- HEIC/HEIF: requires the system "HEIF Image Extensions" codec; files are skipped if decoding fails
- Undecodable RAW files fall back to their embedded preview

Known limitations:

- Single window, single workspace — no tabs; no auto-updater (download new releases manually from GitHub)
- XMP marks for TIFF/HEIF are written as sidecars only, not embedded
- The settings panel is a single scrolling page, not a tabbed sidebar

## Build

Requirements:

- Windows 10 21H2+ / Windows 11 (x86_64)
- Rust stable toolchain (edition 2024, install via rustup)
- Nothing else: SQLite is statically compiled via rusqlite's bundled feature; no VC++ redistributable or .NET needed

```bash
git clone https://github.com/Cloudldust/Kaka
cd kaka
cargo build --release
```

The output is a single distributable executable at `target/release/kaka.exe`.

Note: this repo's `.gitignore` excludes `.cargo/`. If you maintain a machine-local `.cargo/config.toml` (e.g. to redirect `target-dir` to another drive), it applies only to your machine and is not committed — builds from a fresh clone use the default `target/` directory.

Development:

```bash
cargo run      # build and run in debug mode (the default feature set already includes the GUI)
cargo test     # run all tests (unit + integration)
cargo check    # type-check only
```

## Configuration

No environment variables or API keys are needed. All user settings live in `%APPDATA%/Kaka/config.toml`, written by the settings UI but also editable by hand. Example:

```toml
language = "zh"                     # UI language: zh / en
auto_open_last_workspace = true     # reopen the last workspace on startup
auto_detect_card = true             # auto-open the import window when a card is detected
default_target_dir = "D:/Photos"    # default target folder for copy-mode imports
cache_dir = "D:/KakaCache"          # cache root (default %LOCALAPPDATA%/Kaka/cache)
cache_capacity_gb = 20              # disk cache capacity limit
cache_expire_days = 30              # cache expiry in days
star_rating = 3                     # rating written into XMP marks
pair_time_threshold_secs = 5        # RAW+JPG pairing time-delta threshold (1–30 s)
lr_install_path = ""                # path to Lightroom.exe or its folder; empty = auto-detect
export_space_guard = true           # free-space pre-check before export copy
include_sidecar_export = true       # include original XMP sidecars when copying
github_repo = "https://github.com/Cloudldust/Kaka"  # used by the "Open GitHub repository" button

[keybindings]                       # overrides: action code -> key code; missing entries use built-in defaults
mark_delete = "Q"
mark_reviewed = "E"
rotate_cw = "R"
next_photo = "ArrowRight"
```

Other data locations:

| Content | Path |
|---------|------|
| Database | `%APPDATA%/Kaka/kaka.db` |
| Config | `%APPDATA%/Kaka/config.toml` |
| Logs | `%APPDATA%/Kaka/logs/` |
| Cache | `%LOCALAPPDATA%/Kaka/cache/` (overridable via `cache_dir`) |

Remappable action codes: `mark_delete`, `mark_reviewed`, `mark_untreated`, `rotate_cw`, `next_photo`, `prev_photo`, `toggle_zoom`, `toggle_panel`, `select_all`, `undo`, `redo`, `save`.

## License

MIT License — see [LICENSE](LICENSE).
