//! `.le3` file persistence (SPEC §7).
//!
//! A `.le3` is a SQLite database. This early version implements only a small
//! part of the schema from the spec — enough to round-trip sketch primitives —
//! but keeps the pieces that matter for forward compatibility: a `meta` table
//! and a `schema_migrations` table, and shapes stored as DAG nodes with a
//! JSON payload (SPEC §7.3). When real features arrive they slot into the same
//! `dag_nodes` shape.

use crate::model::{Document, Line, Rect, ShapeKind};
use rusqlite::Connection;

/// Bump when the on-disk schema changes; pair with a migration below.
const SCHEMA_VERSION: i64 = 1;
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub type Result<T> = std::result::Result<T, String>;

/// Create the tables for a fresh database (idempotent).
fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_migrations (
            id         INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            applied_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT
        );
        CREATE TABLE IF NOT EXISTS dag_nodes (
            id           INTEGER PRIMARY KEY,
            component_id INTEGER,
            kind         TEXT NOT NULL,
            payload      TEXT NOT NULL
        );
        ",
    )?;
    Ok(())
}

/// Save `doc` to `path`, overwriting any existing document content.
///
/// We rewrite the node table wholesale: at this scale it's simplest and keeps
/// the file an exact reflection of the in-memory document. The action DAG
/// (SPEC §4) will replace this with incremental, append-only history later.
pub fn save(path: &str, doc: &Document) -> Result<()> {
    let mut conn = Connection::open(path).map_err(|e| e.to_string())?;
    init_schema(&conn).map_err(|e| e.to_string())?;

    let tx = conn.transaction().map_err(|e| e.to_string())?;

    tx.execute(
        "INSERT OR REPLACE INTO schema_migrations (id, name, applied_at)
         VALUES (?1, 'initial', datetime('now'))",
        rusqlite::params![SCHEMA_VERSION],
    )
    .map_err(|e| e.to_string())?;

    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('app_version', ?1)",
        rusqlite::params![APP_VERSION],
    )
    .map_err(|e| e.to_string())?;

    tx.execute(
        "DELETE FROM dag_nodes WHERE kind IN ('rectangle', 'line')",
        [],
    )
    .map_err(|e| e.to_string())?;

    let mut rect_i = 0usize;
    let mut line_i = 0usize;
    for (id, kind) in doc.shape_order.iter().enumerate() {
        match kind {
            ShapeKind::Rect => {
                let rect = doc
                    .rects
                    .get(rect_i)
                    .ok_or_else(|| "shape_order out of sync with rects".to_string())?;
                let payload = serde_json::to_string(rect).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO dag_nodes (id, component_id, kind, payload)
                     VALUES (?1, 0, 'rectangle', ?2)",
                    rusqlite::params![id as i64, payload],
                )
                .map_err(|e| e.to_string())?;
                rect_i += 1;
            }
            ShapeKind::Line => {
                let line = doc
                    .lines
                    .get(line_i)
                    .ok_or_else(|| "shape_order out of sync with lines".to_string())?;
                let payload = serde_json::to_string(line).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO dag_nodes (id, component_id, kind, payload)
                     VALUES (?1, 0, 'line', ?2)",
                    rusqlite::params![id as i64, payload],
                )
                .map_err(|e| e.to_string())?;
                line_i += 1;
            }
            ShapeKind::ConstructionPlane => {}
        }
    }

    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// Open the document stored at `path`.
pub fn open(path: &str) -> Result<Document> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(
            "SELECT kind, payload FROM dag_nodes
             WHERE kind IN ('rectangle', 'line')
             ORDER BY id",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;

    let mut rects = Vec::new();
    let mut lines = Vec::new();
    let mut shape_order = Vec::new();
    for row in rows {
        let (kind, payload) = row.map_err(|e| e.to_string())?;
        match kind.as_str() {
            "rectangle" => {
                let rect: Rect = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                rects.push(rect);
                shape_order.push(ShapeKind::Rect);
            }
            "line" => {
                let line: Line = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                lines.push(line);
                shape_order.push(ShapeKind::Line);
            }
            _ => {}
        }
    }

    Ok(Document {
        rects,
        lines,
        construction_planes: Vec::new(),
        shape_order,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_rectangles() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_roundtrip_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let doc = Document {
            rects: vec![
                Rect { x: 1.0, y: 2.0, w: 3.0, h: 4.0 },
                Rect { x: 10.0, y: 20.0, w: 30.0, h: 40.0 },
            ],
            lines: vec![],
            construction_planes: vec![],
            shape_order: vec![ShapeKind::Rect, ShapeKind::Rect],
        };

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();

        assert_eq!(loaded.rects, doc.rects);
        assert_eq!(loaded.shape_order, doc.shape_order);

        save(&path, &doc).unwrap();
        let reloaded = open(&path).unwrap();
        assert_eq!(reloaded.rects.len(), 2);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn construction_planes_are_not_exported() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_construction_skip_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let doc = Document {
            rects: vec![Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            }],
            lines: vec![],
            construction_planes: vec![crate::model::ConstructionPlane {
                origin: glam::Vec3::new(0.0, 0.0, 25.0),
                normal: glam::Vec3::Z,
                u_axis: glam::Vec3::X,
                v_axis: glam::Vec3::Y,
            }],
            shape_order: vec![ShapeKind::Rect, ShapeKind::ConstructionPlane],
        };

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.rects.len(), 1);
        assert!(loaded.construction_planes.is_empty());
        assert_eq!(loaded.shape_order, vec![ShapeKind::Rect]);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_mixed_shapes_in_order() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_mixed_shapes_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let doc = Document {
            rects: vec![Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            }],
            lines: vec![
                Line::from_endpoints(0.0, 0.0, 5.0, 0.0),
                Line::from_endpoints(1.0, 1.0, 1.0, 6.0),
            ],
            construction_planes: vec![],
            shape_order: vec![ShapeKind::Rect, ShapeKind::Line, ShapeKind::Line],
        };

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded, doc);

        std::fs::remove_file(&path).unwrap();
    }
}