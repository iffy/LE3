//! `.le3` file persistence (SPEC §7).
//!
//! A `.le3` is a SQLite database. This early version implements only a small
//! part of the schema from the spec — enough to round-trip sketch primitives —
//! but keeps the pieces that matter for forward compatibility: a `meta` table
//! and a `schema_migrations` table, and shapes stored as DAG nodes with a
//! JSON payload (SPEC §7.3). When real features arrive they slot into the same
//! `dag_nodes` shape.

use crate::face::default_xy_plane;
use crate::constraints::{migrate_legacy_dimensions, solve_document_constraints};
use crate::model::{
    Circle, ConstructionPlane, Constraint, Document, FaceId, Line, Parameter, Rect, ShapeKind,
    Sketch,
};
use crate::parameters::validate_document_parameters_no_cycles;
use rusqlite::Connection;

/// Bump when the on-disk schema changes; pair with a migration below.
const SCHEMA_VERSION: i64 = 1;
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const CONSTRUCTION_PLANES_META_KEY: &str = "construction_planes";

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
pub fn save(path: &str, doc: &Document) -> Result<()> {
    validate_document_parameters_no_cycles(doc)?;
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

    let planes_payload =
        serde_json::to_string(&doc.construction_planes).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![CONSTRUCTION_PLANES_META_KEY, planes_payload],
    )
    .map_err(|e| e.to_string())?;

    tx.execute(
        "DELETE FROM dag_nodes WHERE kind IN ('sketch', 'rectangle', 'line', 'circle', 'parameter', 'constraint', 'construction_plane')",
        [],
    )
    .map_err(|e| e.to_string())?;

    let mut sketch_i = 0usize;
    let mut rect_i = 0usize;
    let mut line_i = 0usize;
    let mut circle_i = 0usize;
    let mut constraint_i = 0usize;
    let mut param_i = 0usize;
    let mut plane_i = 1usize;
    for (id, kind) in doc.shape_order.iter().enumerate() {
        match kind {
            ShapeKind::Sketch => {
                let sketch = doc
                    .sketches
                    .get(sketch_i)
                    .ok_or_else(|| "shape_order out of sync with sketches".to_string())?;
                let payload = serde_json::to_string(sketch).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO dag_nodes (id, component_id, kind, payload)
                     VALUES (?1, 0, 'sketch', ?2)",
                    rusqlite::params![id as i64, payload],
                )
                .map_err(|e| e.to_string())?;
                sketch_i += 1;
            }
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
            ShapeKind::Circle => {
                let circle = doc
                    .circles
                    .get(circle_i)
                    .ok_or_else(|| "shape_order out of sync with circles".to_string())?;
                let payload = serde_json::to_string(circle).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO dag_nodes (id, component_id, kind, payload)
                     VALUES (?1, 0, 'circle', ?2)",
                    rusqlite::params![id as i64, payload],
                )
                .map_err(|e| e.to_string())?;
                circle_i += 1;
            }
            ShapeKind::Parameter => {
                let param = doc
                    .parameters
                    .get(param_i)
                    .ok_or_else(|| "shape_order out of sync with parameters".to_string())?;
                let payload = serde_json::to_string(param).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO dag_nodes (id, component_id, kind, payload)
                     VALUES (?1, 0, 'parameter', ?2)",
                    rusqlite::params![id as i64, payload],
                )
                .map_err(|e| e.to_string())?;
                param_i += 1;
            }
            ShapeKind::Constraint => {
                let constraint = doc
                    .constraints
                    .get(constraint_i)
                    .ok_or_else(|| "shape_order out of sync with constraints".to_string())?;
                let payload = serde_json::to_string(constraint).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO dag_nodes (id, component_id, kind, payload)
                     VALUES (?1, 0, 'constraint', ?2)",
                    rusqlite::params![id as i64, payload],
                )
                .map_err(|e| e.to_string())?;
                constraint_i += 1;
            }
            ShapeKind::ConstructionPlane => {
                let plane = doc
                    .construction_planes
                    .get(plane_i)
                    .ok_or_else(|| {
                        "shape_order out of sync with construction_planes".to_string()
                    })?;
                let payload = serde_json::to_string(plane).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO dag_nodes (id, component_id, kind, payload)
                     VALUES (?1, 0, 'construction_plane', ?2)",
                    rusqlite::params![id as i64, payload],
                )
                .map_err(|e| e.to_string())?;
                plane_i += 1;
            }
        }
    }

    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

fn load_construction_planes(
    conn: &Connection,
    dag_planes: Vec<ConstructionPlane>,
) -> Result<Vec<ConstructionPlane>> {
    if let Ok(payload) = conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        rusqlite::params![CONSTRUCTION_PLANES_META_KEY],
        |row| row.get::<_, String>(0),
    ) {
        if let Ok(planes) = serde_json::from_str::<Vec<ConstructionPlane>>(&payload) {
            if !planes.is_empty() {
                return Ok(planes);
            }
        }
    }
    let mut planes = vec![default_xy_plane()];
    planes.extend(dag_planes);
    Ok(planes)
}

/// Ensure every sketch-hosted construction-plane index exists after load.
fn ensure_construction_plane_indices(doc: &mut Document) {
    if doc.construction_planes.is_empty() {
        doc.construction_planes.push(default_xy_plane());
    }
    let max_index = doc
        .sketches
        .iter()
        .filter_map(|sketch| match sketch.face {
            FaceId::ConstructionPlane(index) => Some(index),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    while doc.construction_planes.len() <= max_index {
        doc.construction_planes.push(default_xy_plane());
    }
}

/// Open the document stored at `path`.
pub fn open(path: &str) -> Result<Document> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(
            "SELECT kind, payload FROM dag_nodes
             WHERE kind IN ('sketch', 'rectangle', 'line', 'circle', 'parameter', 'constraint', 'construction_plane')
             ORDER BY id",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;

    let mut parameters = Vec::new();
    let mut sketches = Vec::new();
    let mut rects = Vec::new();
    let mut lines = Vec::new();
    let mut circles = Vec::new();
    let mut constraints = Vec::new();
    let mut construction_planes = Vec::new();
    let mut shape_order = Vec::new();
    for row in rows {
        let (kind, payload) = row.map_err(|e| e.to_string())?;
        match kind.as_str() {
            "sketch" => {
                let sketch: Sketch = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                sketches.push(sketch);
                shape_order.push(ShapeKind::Sketch);
            }
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
            "circle" => {
                let circle: Circle = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                circles.push(circle);
                shape_order.push(ShapeKind::Circle);
            }
            "parameter" => {
                let param: Parameter = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                parameters.push(param);
                shape_order.push(ShapeKind::Parameter);
            }
            "constraint" => {
                let constraint: Constraint =
                    serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                constraints.push(constraint);
                shape_order.push(ShapeKind::Constraint);
            }
            "construction_plane" => {
                let plane: ConstructionPlane =
                    serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                construction_planes.push(plane);
                shape_order.push(ShapeKind::ConstructionPlane);
            }
            _ => {}
        }
    }

    let construction_planes =
        load_construction_planes(&conn, construction_planes).map_err(|e| e.to_string())?;

    let mut doc = Document {
        parameters,
        sketches,
        rects,
        lines,
        circles,
        constraints,
        construction_planes,
        shape_order,
    };
    ensure_construction_plane_indices(&mut doc);
    migrate_legacy_dimensions(&mut doc);
    solve_document_constraints(&mut doc).map_err(|e| e.to_string())?;
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Circle, FaceId, RectEdge};

    fn plane_sketch(doc: &mut Document) -> usize {
        doc.add_sketch(FaceId::ConstructionPlane(0))
    }

    #[test]
    fn round_trips_rectangle_dimension_label_offsets() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_rect_dim_offset_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document {
            parameters: Vec::new(),
            sketches: Vec::new(),
            rects: Vec::new(),
            lines: vec![],
            circles: Vec::new(),
            constraints: Vec::new(),
            construction_planes: vec![default_xy_plane()],
            shape_order: Vec::new(),
        };
        let sketch = plane_sketch(&mut doc);
        let mut rect = Rect::from_local_corners(sketch, 0.0, 0.0, 50.8, 5.0);
        rect.width_locked = true;
        rect.height_locked = true;
        rect.width_dim_offset = Some(42.0);
        rect.height_dim_offset = Some(36.0);
        doc.rects.push(rect);
        doc.shape_order.push(ShapeKind::Rect);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.rects[0].width_dim_offset, Some(42.0));
        assert_eq!(loaded.rects[0].height_dim_offset, Some(36.0));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_rectangle_dimension_locks() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_rect_locks_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document {
            parameters: Vec::new(),
            sketches: Vec::new(),
            rects: Vec::new(),
            lines: vec![],
            circles: Vec::new(),
            constraints: Vec::new(),
            construction_planes: vec![default_xy_plane()],
            shape_order: Vec::new(),
        };
        let sketch = plane_sketch(&mut doc);
        let mut rect = Rect::from_local_corners(sketch, 0.0, 0.0, 50.8, 5.0);
        rect.width_locked = true;
        rect.height_locked = false;
        doc.rects.push(rect);
        doc.shape_order.push(ShapeKind::Rect);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert!(loaded.rects[0].width_locked);
        assert!(!loaded.rects[0].height_locked);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_rectangles() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_roundtrip_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document {
            parameters: Vec::new(),
            sketches: Vec::new(),
            rects: Vec::new(),
            lines: vec![],
            circles: Vec::new(),
            constraints: Vec::new(),
            construction_planes: vec![default_xy_plane()],
            shape_order: Vec::new(),
        };
        let sketch = plane_sketch(&mut doc);
        doc.rects.push(Rect::from_local_corners(sketch, 1.0, 2.0, 4.0, 6.0));
        doc.shape_order.push(ShapeKind::Rect);
        doc.rects
            .push(Rect::from_local_corners(sketch, 10.0, 20.0, 40.0, 60.0));
        doc.shape_order.push(ShapeKind::Rect);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();

        assert_eq!(loaded.rects, doc.rects);
        assert_eq!(loaded.shape_order, doc.shape_order);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_rectangle_edge_construction_flags() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_rect_edge_construction_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        let sketch = doc.add_sketch(crate::model::FaceId::ConstructionPlane(0));
        let mut rect = Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0);
        rect.set_edge_construction(RectEdge::Bottom, true);
        rect.set_edge_construction(RectEdge::Top, true);
        doc.rects.push(rect);
        doc.shape_order.push(ShapeKind::Rect);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert!(loaded.rects[0].edge_construction(RectEdge::Bottom));
        assert!(!loaded.rects[0].edge_construction(RectEdge::Right));
        assert!(loaded.rects[0].edge_construction(RectEdge::Top));
        assert!(!loaded.rects[0].edge_construction(RectEdge::Left));

        std::fs::remove_file(&path).unwrap();
    }

    fn element_world_anchors(doc: &Document) -> Vec<glam::Vec3> {
        let mut anchors = Vec::new();
        for plane in &doc.construction_planes {
            anchors.push(plane.origin);
        }
        for rect in &doc.rects {
            anchors.push(crate::face::rect_world_corners(doc, rect).unwrap()[0]);
        }
        for circle in &doc.circles {
            anchors.push(crate::face::circle_world_center(doc, circle).unwrap());
        }
        for line in &doc.lines {
            let (a, b) = crate::face::line_world_endpoints(doc, line).unwrap();
            anchors.push(a);
            anchors.push(b);
        }
        anchors
    }

    fn assert_world_anchors_match(before: &[glam::Vec3], after: &[glam::Vec3]) {
        assert_eq!(
            before.len(),
            after.len(),
            "element world anchor count should match after reload"
        );
        for (a, b) in before.iter().zip(after) {
            assert!(
                (*a - *b).length() < 1e-3,
                "world anchor {:?} should round-trip as {:?}",
                a,
                b
            );
        }
    }

    #[test]
    fn world_positions_round_trip_through_save() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_world_positions_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let offset_plane = crate::construction::plane_from_definition(
            &crate::construction::definition_from_reference(
                &crate::construction::PlaneReference::Face {
                    origin: glam::Vec3::ZERO,
                    normal: glam::Vec3::Z,
                    label: "Ground".to_string(),
                },
                25.0,
                0.0,
            ),
            crate::model::ConstructionPlaneParent::Root,
        );
        let mut doc = Document {
            parameters: Vec::new(),
            sketches: Vec::new(),
            rects: Vec::new(),
            lines: vec![],
            circles: Vec::new(),
            constraints: Vec::new(),
            construction_planes: vec![default_xy_plane(), offset_plane],
            shape_order: Vec::new(),
        };

        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.circles.push(Circle::from_local_center_radius(
            s0, 12.0, -8.0, 15.0, 0.4,
        ));
        doc.shape_order.push(ShapeKind::Circle);

        let s1 = doc.add_sketch(FaceId::ConstructionPlane(1));
        doc.rects
            .push(Rect::from_local_corners(s1, 3.0, 4.0, 13.0, 14.0));
        doc.lines.push(Line::from_local_endpoints(
            s1, -2.0, 1.0, 8.0, 6.0,
        ));
        doc.shape_order.push(ShapeKind::Rect);
        doc.shape_order.push(ShapeKind::Line);
        doc.shape_order.push(ShapeKind::ConstructionPlane);

        let s2 = doc.add_sketch(FaceId::Rect(0));
        doc.rects
            .push(Rect::from_local_corners(s2, 1.0, 2.0, 4.0, 5.0));
        doc.shape_order.push(ShapeKind::Rect);

        let before = element_world_anchors(&doc);
        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        let after = element_world_anchors(&loaded);
        assert_world_anchors_match(&before, &after);

        let offset_rect = crate::face::rect_world_corners(&loaded, &loaded.rects[0]).unwrap();
        assert!(
            (offset_rect[0].z - 25.0).abs() < 1e-3,
            "rectangle on offset plane should keep its world height"
        );

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn default_construction_plane_origin_round_trips() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_plane0_origin_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.construction_planes[0].origin.z = 30.0;
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 10.0));
        doc.shape_order.push(ShapeKind::Rect);

        let before_origin = doc.construction_planes[0].origin;
        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert!(
            (loaded.construction_planes[0].origin - before_origin).length() < 1e-3,
            "edited default plane origin should round-trip"
        );

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn construction_planes_round_trip() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_construction_plane_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let offset_plane = crate::construction::plane_from_definition(
            &crate::construction::definition_from_reference(
                &crate::construction::PlaneReference::Face {
                    origin: glam::Vec3::ZERO,
                    normal: glam::Vec3::Z,
                    label: "Ground".to_string(),
                },
                25.0,
                0.0,
            ),
            crate::model::ConstructionPlaneParent::Root,
        );
        let mut doc = Document {
            parameters: Vec::new(),
            sketches: Vec::new(),
            rects: Vec::new(),
            lines: vec![],
            circles: Vec::new(),
            constraints: Vec::new(),
            construction_planes: vec![default_xy_plane(), offset_plane.clone()],
            shape_order: Vec::new(),
        };
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(1));
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 10.0));
        doc.shape_order.push(ShapeKind::Rect);
        doc.shape_order.push(ShapeKind::ConstructionPlane);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.rects.len(), 1);
        assert_eq!(loaded.construction_planes.len(), 2);
        assert_eq!(loaded.construction_planes[1], offset_plane);
        assert_eq!(
            loaded.sketches[0].face,
            FaceId::ConstructionPlane(1),
            "rectangle sketch should stay on the offset plane"
        );
        let corners = crate::face::rect_world_corners(&loaded, &loaded.rects[0]).unwrap();
        assert!(
            (corners[0].z - 25.0).abs() < 1e-3,
            "loaded rectangle should keep its offset-plane world position"
        );

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn legacy_files_without_planes_get_placeholder_indices() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_legacy_plane_ref_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let doc = Document {
            parameters: Vec::new(),
            sketches: vec![Sketch {
                face: FaceId::ConstructionPlane(1),
                name: None,
            }],
            rects: vec![Rect::from_local_corners(0, 0.0, 0.0, 10.0, 10.0)],
            lines: vec![],
            circles: Vec::new(),
            constraints: Vec::new(),
            construction_planes: vec![default_xy_plane()],
            shape_order: vec![ShapeKind::Sketch, ShapeKind::Rect],
        };
        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert!(
            loaded.construction_planes.len() >= 2,
            "legacy sketch references to plane 1 should not crash on load"
        );
        assert!(
            crate::face::rect_world_corners(&loaded, &loaded.rects[0]).is_some()
        );

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_mixed_shapes_in_order() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_mixed_shapes_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document {
            parameters: Vec::new(),
            sketches: Vec::new(),
            rects: Vec::new(),
            lines: Vec::new(),
            circles: Vec::new(),
            constraints: Vec::new(),
            construction_planes: vec![default_xy_plane()],
            shape_order: Vec::new(),
        };
        let sketch = plane_sketch(&mut doc);
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 10.0));
        doc.shape_order.push(ShapeKind::Rect);
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 5.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.lines
            .push(Line::from_local_endpoints(sketch, 1.0, 1.0, 1.0, 6.0));
        doc.shape_order.push(ShapeKind::Line);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.rects, doc.rects);
        assert_eq!(loaded.lines, doc.lines);
        assert_eq!(loaded.shape_order, doc.shape_order);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_circles() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_circle_roundtrip_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        let mut circle = Circle::from_local_center_radius(sketch, 5.0, 5.0, 10.0, 0.5);
        circle.diameter_dim_offset = Some(18.0);
        circle.diameter_dim_angle = 1.2;
        circle.construction = true;
        doc.circles.push(circle);
        doc.shape_order.push(ShapeKind::Circle);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.circles, doc.circles);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_sketches() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_sketch_roundtrip.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        let s1 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(s0, 0.0, 0.0, 1.0, 1.0));
        doc.shape_order.push(ShapeKind::Rect);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.sketches.len(), 2);
        assert_eq!(loaded.sketches[0].face, FaceId::ConstructionPlane(0));
        assert_eq!(loaded.sketches[1].face, FaceId::ConstructionPlane(0));
        assert_eq!(loaded.rects[0].sketch, s0);
        let _ = s1;

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn save_rejects_circular_parameters() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_circular_params_test.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        doc.parameters.push(Parameter {
            name: "A".to_string(),
            expression: "B".to_string(),
        });
        doc.parameters.push(Parameter {
            name: "B".to_string(),
            expression: "A".to_string(),
        });
        doc.shape_order.push(ShapeKind::Parameter);
        doc.shape_order.push(ShapeKind::Parameter);

        let err = save(&path, &doc).unwrap_err();
        assert!(err.contains("Circular dependency"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn round_trips_parameters() {
        let dir = std::env::temp_dir();
        let path = dir.join("le3_parameters_roundtrip.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        doc.parameters.push(Parameter {
            name: "A".to_string(),
            expression: "5mm".to_string(),
        });
        doc.parameters.push(Parameter {
            name: "B".to_string(),
            expression: "A + 5in".to_string(),
        });
        doc.shape_order.push(ShapeKind::Parameter);
        doc.shape_order.push(ShapeKind::Parameter);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.parameters, doc.parameters);
        assert_eq!(loaded.shape_order, doc.shape_order);

        std::fs::remove_file(&path).unwrap();
    }
}