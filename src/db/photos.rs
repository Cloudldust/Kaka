//! Photo record CRUD and filtering queries (PRD 10.2).

use super::Db;
use crate::model::*;
use rusqlite::{params, Row};

pub const STATUS_UNTREATED: i64 = 0;
pub const STATUS_DELETE: i64 = 1;
pub const STATUS_REVIEWED: i64 = 2;

fn map_photo(r: &Row) -> rusqlite::Result<Photo> {
    Ok(Photo {
        id: r.get("id")?,
        original_filename: r.get("original_filename")?,
        file_size: r.get("file_size")?,
        capture_time: r.get("capture_time")?,
        current_path: r.get("current_path")?,
        folder_path: r.get("folder_path")?,
        status: Status::from_i64(r.get("status")?),
        thumb_hash: r.get("thumb_hash")?,
        decode_failed: r.get::<_, i64>("decode_failed")? != 0,
        preview_only: r.get::<_, i64>("preview_only")? != 0,
        rotation_override: r.get("rotation_override")?,
        exif_orientation: r.get("exif_orientation")?,
        pair_group_id: r.get("pair_group_id")?,
        iso: r.get("iso")?,
        aperture: r.get("aperture")?,
        shutter_speed: r.get("shutter_speed")?,
        focal_length: r.get("focal_length")?,
        camera_model: r.get("camera_model")?,
        lens_model: r.get("lens_model")?,
        capture_time_source: r.get("capture_time_source")?,
        import_time: r.get("import_time")?,
        last_access_time: r.get("last_access_time")?,
        marked_delete_time: r.get("marked_delete_time")?,
        marked_review_time: r.get("marked_review_time")?,
    })
}

fn map_list_item(r: &Row) -> rusqlite::Result<PhotoListItem> {
    Ok(PhotoListItem {
        id: r.get("id")?,
        original_filename: r.get("original_filename")?,
        current_path: r.get("current_path")?,
        folder_path: r.get("folder_path")?,
        status: Status::from_i64(r.get("status")?),
        capture_time: r.get("capture_time")?,
        file_size: r.get("file_size")?,
        thumb_hash: r.get("thumb_hash")?,
        camera_model: r.get("camera_model")?,
        lens_model: r.get("lens_model")?,
        iso: r.get("iso")?,
        aperture: r.get("aperture")?,
        shutter_speed: r.get("shutter_speed")?,
        focal_length: r.get("focal_length")?,
        decode_failed: r.get::<_, i64>("decode_failed")? != 0,
        preview_only: r.get::<_, i64>("preview_only")? != 0,
        pair_group_id: r.get("pair_group_id")?,
        rotation_override: r.get("rotation_override")?,
    })
}

/// Attempt to insert a photo. Returns Ok(Some(id)) when inserted,
/// Ok(None) when the unique constraint caused a silent skip (ON CONFLICT IGNORE).
pub fn insert_photo(db: &Db, p: &Photo) -> anyhow::Result<Option<i64>> {
    let n = db
        .conn
        .execute(
            "INSERT INTO photos
             (original_filename, file_size, capture_time, current_path, folder_path,
              status, thumb_hash, decode_failed, preview_only, rotation_override,
              exif_orientation, pair_group_id, iso, aperture, shutter_speed, focal_length,
              camera_model, lens_model, capture_time_source, import_time, last_access_time,
              marked_delete_time, marked_review_time)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,
                     datetime('now'), datetime('now'), ?20, ?21)
             ON CONFLICT(original_filename, file_size, capture_time) DO NOTHING",
            params![
                p.original_filename,
                p.file_size,
                p.capture_time,
                p.current_path,
                p.folder_path,
                p.status.as_i64(),
                p.thumb_hash,
                p.decode_failed as i64,
                p.preview_only as i64,
                p.rotation_override,
                p.exif_orientation,
                p.pair_group_id,
                p.iso,
                p.aperture,
                p.shutter_speed,
                p.focal_length,
                p.camera_model,
                p.lens_model,
                p.capture_time_source,
                p.marked_delete_time,
                p.marked_review_time,
            ],
        )?;
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(db.conn.last_insert_rowid()))
}

/// Look up the id of an existing photo matching the dedup three elements.
pub fn find_by_three_elements(
    db: &Db,
    filename: &str,
    file_size: i64,
    capture_time: &str,
) -> anyhow::Result<Option<Photo>> {
    let mut stmt = db.conn.prepare(
        "SELECT * FROM photos
         WHERE original_filename = ?1 AND file_size = ?2 AND capture_time = ?3
         LIMIT 1",
    )?;
    let mut rows = stmt.query(params![filename, file_size, capture_time])?;
    if let Some(row) = rows.next()? {
        Ok(Some(map_photo(row)?))
    } else {
        Ok(None)
    }
}

/// Get a single photo by id.
pub fn get_photo(db: &Db, id: i64) -> anyhow::Result<Option<Photo>> {
    let mut stmt = db.conn.prepare("SELECT * FROM photos WHERE id = ?1")?;
    let mut rows = stmt.query(params![id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(map_photo(row)?))
    } else {
        Ok(None)
    }
}

/// Update `current_path` (path repair, PRD 6.4). Returns rows updated.
pub fn update_path(db: &Db, id: i64, new_path: &str, folder_path: &str) -> anyhow::Result<usize> {
    let n = db.conn.execute(
        "UPDATE photos SET current_path = ?1, folder_path = ?2 WHERE id = ?3",
        params![new_path, folder_path, id],
    )?;
    Ok(n)
}

/// Update a photo's status, setting the corresponding marker timestamp.
/// Returns the number of rows changed (0 if the status was already set).
pub fn set_status(db: &Db, id: i64, status: Status) -> anyhow::Result<usize> {
    let n = db.conn.execute(
        "UPDATE photos SET status = ?1,
         marked_delete_time = CASE WHEN ?1 = 1 THEN datetime('now') ELSE NULL END,
         marked_review_time = CASE WHEN ?1 = 2 THEN datetime('now') ELSE NULL END
         WHERE id = ?2",
        params![status.as_i64(), id],
    )?;
    Ok(n)
}

/// Update the thumbnail hash for a photo.
pub fn set_thumb_hash(db: &Db, id: i64, hash: Option<&str>) -> anyhow::Result<()> {
    db.conn.execute(
        "UPDATE photos SET thumb_hash = ?1 WHERE id = ?2",
        params![hash, id],
    )?;
    Ok(())
}

/// Persist the RAW-decode-failed marker (PRD 7.4.3): failed photos are never
/// retried on the next startup; the right-click 强制重试 clears it first.
pub fn set_decode_failed(db: &Db, id: i64, failed: bool) -> anyhow::Result<()> {
    db.conn.execute(
        "UPDATE photos SET decode_failed = ?1 WHERE id = ?2",
        params![failed as i64, id],
    )?;
    Ok(())
}

/// Update rotation override (0..=3) and exif orientation marker.
pub fn set_rotation(db: &Db, id: i64, rotation_override: i64) -> anyhow::Result<()> {
    db.conn.execute(
        "UPDATE photos SET rotation_override = ?1 WHERE id = ?2",
        params![rotation_override, id],
    )?;
    Ok(())
}

/// Touch a photo's last_access_time.
pub fn touch(db: &Db, id: i64) -> anyhow::Result<()> {
    db.conn.execute(
        "UPDATE photos SET last_access_time = datetime('now') WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

/// Delete a photo record by id.
pub fn delete_photo(db: &Db, id: i64) -> anyhow::Result<()> {
    db.conn.execute("DELETE FROM photos WHERE id = ?1", params![id])?;
    Ok(())
}

/// SQL fragment for the ORDER BY corresponding to a sort order.
pub fn sort_sql(order: SortOrder) -> &'static str {
    match order {
        SortOrder::CaptureTimeAsc => "capture_time ASC, original_filename ASC",
        SortOrder::CaptureTimeDesc => "capture_time DESC, original_filename ASC",
        SortOrder::FilenameAsc => "original_filename ASC, capture_time ASC",
        SortOrder::FilenameDesc => "original_filename DESC, capture_time ASC",
        SortOrder::FileSizeAsc => "file_size ASC, capture_time ASC",
        SortOrder::FileSizeDesc => "file_size DESC, capture_time ASC",
        SortOrder::ImportTimeAsc => "import_time ASC, id ASC",
        SortOrder::ImportTimeDesc => "import_time DESC, id ASC",
        // 未处理→已阅→待删, group ordered by capture time within.
        SortOrder::StatusGrouped => "status ASC, capture_time ASC, original_filename ASC",
    }
}

/// Count photos matching a folder prefix and optional status.
pub fn count_photos(db: &Db, folder_prefix: &str) -> anyhow::Result<i64> {
    db.conn
        .query_row(
            "SELECT COUNT(*) FROM photos WHERE folder_path LIKE ?1 || '%'",
            params![folder_prefix],
            |r| r.get(0),
        )
        .map_err(Into::into)
}

/// List photo ids in a folder, sorted per the given order.
pub fn list_ids_in_folder(
    db: &Db,
    folder_prefix: &str,
    order: SortOrder,
) -> anyhow::Result<Vec<i64>> {
    let sql = format!(
        "SELECT id FROM photos WHERE folder_path LIKE ?1 || '%' ORDER BY {}",
        sort_sql(order)
    );
    let mut stmt = db.conn.prepare(&sql)?;
    let mut rows = stmt.query(params![folder_prefix])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row.get(0)?);
    }
    Ok(out)
}

/// List photo list items in a folder, sorted per the given order.
pub fn list_items_in_folder(
    db: &Db,
    folder_prefix: &str,
    order: SortOrder,
) -> anyhow::Result<Vec<PhotoListItem>> {
    let sql = format!(
        "SELECT * FROM photos WHERE folder_path LIKE ?1 || '%' ORDER BY {}",
        sort_sql(order)
    );
    let mut stmt = db.conn.prepare(&sql)?;
    let mut rows = stmt.query(params![folder_prefix])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(map_list_item(row)?);
    }
    Ok(out)
}

/// List photo list items in a folder matching an advanced filter (PRD 7.8).
/// SQL conditions are applied for the structured fields; format + missing-file
/// checks are applied in memory (they need filename/disk inspection).
pub fn list_items_filtered(
    db: &Db,
    folder_prefix: &str,
    order: SortOrder,
    filter: &crate::model::Filter,
) -> anyhow::Result<Vec<PhotoListItem>> {
    use rusqlite::types::Value;

    let mut sql = String::from("SELECT * FROM photos WHERE folder_path LIKE ?1 || '%'");
    let mut params: Vec<Value> = vec![folder_prefix.to_string().into()];

    if !filter.statuses.is_empty() {
        let ph = filter.statuses.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        sql.push_str(&format!(" AND status IN ({ph})"));
        for s in &filter.statuses {
            params.push((*s).into());
        }
    }
    for (col, vals) in [("camera_model", &filter.cameras), ("lens_model", &filter.lenses)] {
        if !vals.is_empty() {
            let ph = vals.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            sql.push_str(&format!(" AND {col} IN ({ph})"));
            for v in vals {
                params.push(v.to_string().into());
            }
        }
    }
    if let Some(mn) = filter.iso_min {
        sql.push_str(" AND iso >= ?");
        params.push(mn.into());
    }
    if let Some(mx) = filter.iso_max {
        sql.push_str(" AND iso <= ?");
        params.push(mx.into());
    }
    if let Some(mn) = filter.focal_min {
        sql.push_str(" AND focal_length >= ?");
        params.push(mn.into());
    }
    if let Some(mx) = filter.focal_max {
        sql.push_str(" AND focal_length <= ?");
        params.push(mx.into());
    }
    if let Some(d) = &filter.date_from {
        sql.push_str(" AND capture_time >= ?");
        params.push(d.to_string().into());
    }
    if let Some(d) = &filter.date_to {
        // Inclusive: include the whole end day.
        sql.push_str(" AND capture_time <= ?");
        params.push(format!("{d} 23:59:59").into());
    }
    if let Some(true) = filter.pair {
        sql.push_str(" AND pair_group_id IS NOT NULL");
    }
    if let Some(false) = filter.pair {
        sql.push_str(" AND pair_group_id IS NULL");
    }
    sql.push_str(&format!(" ORDER BY {}", sort_sql(order)));

    let mut stmt = db.conn.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(map_list_item(row)?);
    }

    // In-memory filters: format (by extension) and missing-file (disk check).
    if !filter.formats.is_empty() {
        let fmts: Vec<String> = filter.formats.iter().map(|f| f.to_lowercase()).collect();
        out.retain(|p| {
            let ext = std::path::Path::new(&p.original_filename)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            fmts.contains(&ext)
        });
    }
    if let Some(missing) = filter.missing {
        out.retain(|p| p.is_missing() == missing);
    }
    Ok(out)
}
#[derive(Debug, Clone, Default)]
pub struct StatusCounts {
    pub total: i64,
    pub untreated: i64,
    pub deleted: i64,
    pub reviewed: i64,
}

pub fn status_counts(db: &Db, folder_prefix: &str) -> anyhow::Result<StatusCounts> {
    let mut stmt = db.conn.prepare(
        "SELECT
            COUNT(*) AS total,
            SUM(CASE WHEN status = 0 THEN 1 ELSE 0 END) AS untreated,
            SUM(CASE WHEN status = 1 THEN 1 ELSE 0 END) AS deleted,
            SUM(CASE WHEN status = 2 THEN 1 ELSE 0 END) AS reviewed
         FROM photos WHERE folder_path LIKE ?1 || '%'",
    )?;
    let mut rows = stmt.query(params![folder_prefix])?;
    let mut c = StatusCounts::default();
    if let Some(row) = rows.next()? {
        c.total = row.get(0)?;
        c.untreated = row.get::<_, Option<i64>>(1)?.unwrap_or(0);
        c.deleted = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
        c.reviewed = row.get::<_, Option<i64>>(3)?.unwrap_or(0);
    }
    Ok(c)
}

/// Fetch a list item by id (for the preview/current photo).
pub fn get_list_item(db: &Db, id: i64) -> anyhow::Result<Option<PhotoListItem>> {
    let mut stmt = db.conn.prepare("SELECT * FROM photos WHERE id = ?1")?;
    let mut rows = stmt.query(params![id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(map_list_item(row)?))
    } else {
        Ok(None)
    }
}

/// All camera models present in a folder (for the advanced filter).
pub fn distinct_camera_models(db: &Db, folder_prefix: &str) -> anyhow::Result<Vec<String>> {
    distinct_column(db, "camera_model", folder_prefix)
}

/// All lens models present in a folder.
pub fn distinct_lens_models(db: &Db, folder_prefix: &str) -> anyhow::Result<Vec<String>> {
    distinct_column(db, "lens_model", folder_prefix)
}

fn distinct_column(db: &Db, col: &str, folder_prefix: &str) -> anyhow::Result<Vec<String>> {
    let sql = format!(
        "SELECT DISTINCT {col} FROM photos
         WHERE folder_path LIKE ?1 || '%' AND {col} IS NOT NULL AND {col} != ''
         ORDER BY {col}"
    );
    let mut stmt = db.conn.prepare(&sql)?;
    let mut rows = stmt.query(params![folder_prefix])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row.get(0)?);
    }
    Ok(out)
}

/// Batch set all selected photo ids to a status inside a single transaction.
/// Returns the number of photos that were actually changed.
pub fn set_status_batch(db: &Db, ids: &[i64], status: Status) -> anyhow::Result<usize> {
    db.conn.execute("BEGIN IMMEDIATE", [])?;
    let r = (|| -> anyhow::Result<usize> {
        let mut n = 0usize;
        for id in ids {
            n += set_status(db, *id, status)?;
        }
        Ok(n)
    })();
    match r {
        Ok(n) => {
            db.conn.execute("COMMIT", [])?;
            Ok(n)
        }
        Err(e) => {
            let _ = db.conn.execute("ROLLBACK", []);
            Err(e)
        }
    }
}
