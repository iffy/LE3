//! `.bearcad` file persistence (SPEC §7).
//!
//! A `.bearcad` is a SQLite database. This early version implements only a small
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
use crate::value::{AngleUnit, LengthUnit};
use rusqlite::Connection;

/// Bump when the on-disk schema changes; pair with a migration below.
const SCHEMA_VERSION: i64 = 1;
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const CONSTRUCTION_PLANES_META_KEY: &str = "construction_planes";
const SHAPE_ORDER_META_KEY: &str = "shape_order";
/// Document-level default length unit (#52); missing for files saved before this change,
/// which fall back to [`LengthUnit::default`] (mm), matching their pre-existing behaviour.
const DEFAULT_LENGTH_UNIT_META_KEY: &str = "default_length_unit";
/// Document-level default angle unit (#52); missing for files saved before this change,
/// which fall back to [`AngleUnit::default`] (deg), matching their pre-existing behaviour.
const DEFAULT_ANGLE_UNIT_META_KEY: &str = "default_angle_unit";

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

    let shape_order_payload =
        serde_json::to_string(&doc.shape_order).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![SHAPE_ORDER_META_KEY, shape_order_payload],
    )
    .map_err(|e| e.to_string())?;

    let default_length_unit_payload =
        serde_json::to_string(&doc.default_length_unit).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![DEFAULT_LENGTH_UNIT_META_KEY, default_length_unit_payload],
    )
    .map_err(|e| e.to_string())?;

    let default_angle_unit_payload =
        serde_json::to_string(&doc.default_angle_unit).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![DEFAULT_ANGLE_UNIT_META_KEY, default_angle_unit_payload],
    )
    .map_err(|e| e.to_string())?;

    tx.execute(
        "DELETE FROM dag_nodes WHERE kind IN ('sketch', 'rectangle', 'line', 'circle', 'parameter', 'constraint', 'construction_plane', 'extrusion', 'body', 'imported_mesh')",
        [],
    )
    .map_err(|e| e.to_string())?;

    let mut row_id = 0i64;
    save_indexed_nodes(&tx, &mut row_id, "sketch", &doc.sketches)?;
    save_indexed_nodes(&tx, &mut row_id, "rectangle", &doc.rects)?;
    save_indexed_nodes(&tx, &mut row_id, "line", &doc.lines)?;
    save_indexed_nodes(&tx, &mut row_id, "circle", &doc.circles)?;
    save_indexed_nodes(&tx, &mut row_id, "parameter", &doc.parameters)?;
    save_indexed_nodes(&tx, &mut row_id, "constraint", &doc.constraints)?;
    save_indexed_nodes(&tx, &mut row_id, "extrusion", &doc.extrusions)?;
    save_indexed_nodes(&tx, &mut row_id, "body", &doc.bodies)?;
    save_indexed_nodes(&tx, &mut row_id, "imported_mesh", &doc.imported_meshes)?;
    if doc.construction_planes.len() > 1 {
        save_indexed_nodes(
            &tx,
            &mut row_id,
            "construction_plane",
            &doc.construction_planes[1..],
        )?;
    }

    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

fn save_indexed_nodes<T: serde::Serialize>(
    tx: &rusqlite::Transaction<'_>,
    row_id: &mut i64,
    kind: &str,
    entities: &[T],
) -> Result<()> {
    for (index, entity) in entities.iter().enumerate() {
        let payload = serde_json::to_string(entity).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO dag_nodes (id, component_id, kind, payload)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![*row_id, index as i64, kind, payload],
        )
        .map_err(|e| e.to_string())?;
        *row_id += 1;
    }
    Ok(())
}

fn load_shape_order_meta(conn: &Connection) -> Option<Vec<ShapeKind>> {
    let payload: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            rusqlite::params![SHAPE_ORDER_META_KEY],
            |row| row.get(0),
        )
        .ok()?;
    serde_json::from_str(&payload).ok()
}

/// Load the document-level default length unit, falling back to mm for files saved before
/// this key existed (#52).
fn load_default_length_unit_meta(conn: &Connection) -> LengthUnit {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        rusqlite::params![DEFAULT_LENGTH_UNIT_META_KEY],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|payload| serde_json::from_str(&payload).ok())
    .unwrap_or_default()
}

/// Load the document-level default angle unit, falling back to degrees for files saved
/// before this key existed (#52).
fn load_default_angle_unit_meta(conn: &Connection) -> AngleUnit {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        rusqlite::params![DEFAULT_ANGLE_UNIT_META_KEY],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|payload| serde_json::from_str(&payload).ok())
    .unwrap_or_default()
}

fn load_indexed_entities<T: serde::de::DeserializeOwned>(
    conn: &Connection,
    kind: &str,
) -> Result<Vec<T>> {
    let mut stmt = conn
        .prepare(
            "SELECT component_id, payload FROM dag_nodes
             WHERE kind = ?1
             ORDER BY component_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![kind], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut entities = Vec::new();
    for row in rows {
        let (index, payload) = row.map_err(|e| e.to_string())?;
        let index = usize::try_from(index).map_err(|_| format!("bad {kind} index"))?;
        if index != entities.len() {
            return Err(format!(
                "{kind} indices must be dense starting at 0 (expected {}, got {index})",
                entities.len()
            ));
        }
        let entity: T = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
        entities.push(entity);
    }
    Ok(entities)
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

fn load_legacy_document_nodes(
    conn: &Connection,
) -> Result<(
    Vec<Parameter>,
    Vec<Sketch>,
    Vec<Rect>,
    Vec<Line>,
    Vec<Circle>,
    Vec<Constraint>,
    Vec<ConstructionPlane>,
    Vec<ShapeKind>,
)> {
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
    Ok((
        parameters,
        sketches,
        rects,
        lines,
        circles,
        constraints,
        construction_planes,
        shape_order,
    ))
}

/// Open the document stored at `path`.
pub fn open(path: &str) -> Result<Document> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;

    let (
        parameters,
        sketches,
        rects,
        lines,
        circles,
        constraints,
        construction_planes,
        shape_order,
    ) = if let Some(shape_order) = load_shape_order_meta(&conn) {
        let parameters = load_indexed_entities(&conn, "parameter")?;
        let sketches = load_indexed_entities(&conn, "sketch")?;
        let rects = load_indexed_entities(&conn, "rectangle")?;
        let lines = load_indexed_entities(&conn, "line")?;
        let circles = load_indexed_entities(&conn, "circle")?;
        let constraints = load_indexed_entities(&conn, "constraint")?;
        let dag_planes = load_indexed_entities(&conn, "construction_plane")?;
        (
            parameters,
            sketches,
            rects,
            lines,
            circles,
            constraints,
            dag_planes,
            shape_order,
        )
    } else {
        load_legacy_document_nodes(&conn)?
    };

    let construction_planes =
        load_construction_planes(&conn, construction_planes).map_err(|e| e.to_string())?;
    // Extrusions/bodies (empty for legacy files that predate them).
    let extrusions = load_indexed_entities(&conn, "extrusion")?;
    let bodies = load_indexed_entities(&conn, "body")?;
    let imported_meshes = load_indexed_entities(&conn, "imported_mesh")?;
    let default_length_unit = load_default_length_unit_meta(&conn);
    let default_angle_unit = load_default_angle_unit_meta(&conn);

    let mut doc = Document {
        parameters,
        sketches,
        rects,
        lines,
        circles,
        constraints,
        construction_planes,
        extrusions,
        bodies,
        imported_meshes,
        shape_order,
        default_length_unit,
        default_angle_unit,
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
        let path = dir.join("bearcad_rect_dim_offset_test.bearcad");
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
            extrusions: Vec::new(),
            bodies: Vec::new(),
            imported_meshes: Vec::new(),
            shape_order: Vec::new(),
            default_length_unit: LengthUnit::default(),
            default_angle_unit: AngleUnit::default(),
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
        let path = dir.join("bearcad_rect_locks_test.bearcad");
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
            extrusions: Vec::new(),
            bodies: Vec::new(),
            imported_meshes: Vec::new(),
            shape_order: Vec::new(),
            default_length_unit: LengthUnit::default(),
            default_angle_unit: AngleUnit::default(),
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
        let path = dir.join("bearcad_roundtrip_test.bearcad");
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
            extrusions: Vec::new(),
            bodies: Vec::new(),
            imported_meshes: Vec::new(),
            shape_order: Vec::new(),
            default_length_unit: LengthUnit::default(),
            default_angle_unit: AngleUnit::default(),
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
        let path = dir.join("bearcad_rect_edge_construction_test.bearcad");
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
        let path = dir.join("bearcad_world_positions_test.bearcad");
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
            extrusions: Vec::new(),
            bodies: Vec::new(),
            imported_meshes: Vec::new(),
            shape_order: Vec::new(),
            default_length_unit: LengthUnit::default(),
            default_angle_unit: AngleUnit::default(),
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
        let path = dir.join("bearcad_plane0_origin_test.bearcad");
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
        let path = dir.join("bearcad_construction_plane_test.bearcad");
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
            extrusions: Vec::new(),
            bodies: Vec::new(),
            imported_meshes: Vec::new(),
            shape_order: Vec::new(),
            default_length_unit: LengthUnit::default(),
            default_angle_unit: AngleUnit::default(),
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
        let path = dir.join("bearcad_legacy_plane_ref_test.bearcad");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let doc = Document {
            parameters: Vec::new(),
            sketches: vec![Sketch {
                face: FaceId::ConstructionPlane(1),
                name: None,
                deleted: false,
                length_unit: None,
                angle_unit: None,
            }],
            rects: vec![Rect::from_local_corners(0, 0.0, 0.0, 10.0, 10.0)],
            lines: vec![],
            circles: Vec::new(),
            constraints: Vec::new(),
            construction_planes: vec![default_xy_plane()],
            extrusions: Vec::new(),
            bodies: Vec::new(),
            imported_meshes: Vec::new(),
            shape_order: vec![ShapeKind::Sketch, ShapeKind::Rect],
            default_length_unit: LengthUnit::default(),
            default_angle_unit: AngleUnit::default(),
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
        let path = dir.join("bearcad_mixed_shapes_test.bearcad");
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
            extrusions: Vec::new(),
            bodies: Vec::new(),
            imported_meshes: Vec::new(),
            shape_order: Vec::new(),
            default_length_unit: LengthUnit::default(),
            default_angle_unit: AngleUnit::default(),
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
        let path = dir.join("bearcad_circle_roundtrip_test.bearcad");
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
        let path = dir.join("bearcad_sketch_roundtrip.bearcad");
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
    fn round_trips_extrusions_and_bodies() {
        use crate::model::{Body, BodySource, ExtrudeFace, Extrusion};
        let dir = std::env::temp_dir();
        let path = dir.join("bearcad_extrusion_roundtrip.bearcad");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        doc.shape_order.push(ShapeKind::Rect);
        doc.extrusions.push(Extrusion {
            sketch,
            faces: vec![ExtrudeFace::Rect(0)],
            distance: 12.0,
            target: None,
            expression: String::new(),
            name: Some("Boss".to_string()),
            deleted: false,
            edge_treatments: Vec::new(),
        });
        doc.shape_order.push(ShapeKind::Extrusion);
        doc.bodies.push(Body {
            source: BodySource::Extrusion(0),
            name: None,
            deleted: false,
        });
        doc.shape_order.push(ShapeKind::Body);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.extrusions.len(), 1);
        assert_eq!(loaded.extrusions[0].faces, vec![ExtrudeFace::Rect(0)]);
        assert_eq!(loaded.extrusions[0].distance, 12.0);
        assert_eq!(loaded.extrusions[0].name.as_deref(), Some("Boss"));
        assert_eq!(loaded.bodies.len(), 1);
        assert_eq!(loaded.bodies[0].source, BodySource::Extrusion(0));
        assert!(loaded.shape_order.contains(&ShapeKind::Extrusion));
        assert!(loaded.shape_order.contains(&ShapeKind::Body));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_body_with_cut_extrusion() {
        // A `Solid { add, cut }` body (#35): the cut list must survive save/load, and old
        // files (which never carried one) still load as the additive `Extrusion`/`Extrusions`
        // forms thanks to `#[serde(default)]`.
        use crate::model::{Body, BodySource, ExtrudeFace, Extrusion};
        let dir = std::env::temp_dir();
        let path = dir.join("bearcad_cut_body_roundtrip.bearcad");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 10.0));
        doc.rects.push(Rect::from_local_corners(sketch, 3.0, 3.0, 7.0, 7.0));
        for face in [ExtrudeFace::Rect(0), ExtrudeFace::Rect(1)] {
            doc.extrusions.push(Extrusion {
                sketch,
                faces: vec![face],
                distance: 5.0,
                target: None,
                expression: String::new(),
                name: None,
                deleted: false,
                edge_treatments: Vec::new(),
            });
            doc.shape_order.push(ShapeKind::Extrusion);
        }
        doc.bodies.push(Body {
            source: BodySource::Solid {
                add: vec![0],
                cut: vec![1],
            },
            name: None,
            deleted: false,
        });
        doc.shape_order.push(ShapeKind::Body);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(
            loaded.bodies[0].source,
            BodySource::Solid {
                add: vec![0],
                cut: vec![1],
            }
        );
        assert_eq!(loaded.bodies[0].source.extrusion_indices(), [0]);
        assert_eq!(loaded.bodies[0].source.cut_extrusion_indices(), [1]);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn save_rejects_circular_parameters() {
        let dir = std::env::temp_dir();
        let path = dir.join("bearcad_circular_params_test.bearcad");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        doc.parameters.push(Parameter {
            name: "A".to_string(),
            expression: "B".to_string(),
            deleted: false,
            source: None,
        });
        doc.parameters.push(Parameter {
            name: "B".to_string(),
            expression: "A".to_string(),
            deleted: false,
            source: None,
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
        let path = dir.join("bearcad_parameters_roundtrip.bearcad");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        doc.parameters.push(Parameter {
            name: "A".to_string(),
            expression: "5mm".to_string(),
            deleted: false,
            source: None,
        });
        doc.parameters.push(Parameter {
            name: "B".to_string(),
            expression: "A + 5in".to_string(),
            deleted: false,
            source: None,
        });
        doc.shape_order.push(ShapeKind::Parameter);
        doc.shape_order.push(ShapeKind::Parameter);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.parameters, doc.parameters);
        assert_eq!(loaded.shape_order, doc.shape_order);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_tombstoned_entities() {
        let dir = std::env::temp_dir();
        let path = dir.join("bearcad_tombstone_roundtrip.bearcad");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.lines[0].deleted = true;
        doc.parameters.push(Parameter {
            name: "width".to_string(),
            expression: "10mm".to_string(),
            deleted: true,
            source: None,
        });
        doc.shape_order.push(ShapeKind::Parameter);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert!(loaded.lines[0].deleted);
        assert!(loaded.parameters[0].deleted);
        assert_eq!(loaded.lines.len(), 1);
        assert_eq!(loaded.parameters.len(), 1);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_chamfer_fillet_parent_on_a_bridging_line() {
        // #76: `Line::chamfer_fillet_parent` is a `#[serde(default)]` field on an entity
        // already persisted generically via `dag_nodes` JSON payloads, so it should round-trip
        // with no `storage.rs` changes — verify that assumption rather than just trusting it.
        let dir = std::env::temp_dir();
        let path = dir.join("bearcad_chamfer_fillet_parent_roundtrip.bearcad");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.lines.push(Line::from_local_endpoints(sketch, 10.0, 0.0, 10.0, 10.0));
        doc.shape_order.push(ShapeKind::Line);
        let mut bridge = Line::from_local_endpoints(sketch, 7.0, 0.0, 10.0, 3.0);
        bridge.chamfer_fillet_parent = Some(0);
        doc.lines.push(bridge);
        doc.shape_order.push(ShapeKind::Line);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.lines.len(), 3);
        assert_eq!(loaded.lines[0].chamfer_fillet_parent, None);
        assert_eq!(loaded.lines[1].chamfer_fillet_parent, None);
        assert_eq!(loaded.lines[2].chamfer_fillet_parent, Some(0));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_tombstoned_line_with_alive_sibling() {
        use crate::document_lifecycle::tombstone_element;
        use crate::hierarchy::SceneElement;
        use crate::model::{Constraint, ConstraintKind, ConstraintLine};

        let dir = std::env::temp_dir();
        let path = dir.join("bearcad_tombstone_sibling.bearcad");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 5.0, 10.0, 5.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.constraints.push(Constraint {
            sketch,
            kind: ConstraintKind::Parallel {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        doc.shape_order.push(ShapeKind::Constraint);
        tombstone_element(&mut doc, SceneElement::Line(0));

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.lines.len(), 2);
        assert!(loaded.lines[0].deleted);
        assert!(!loaded.lines[1].deleted);
        assert_eq!(loaded.constraints.len(), 1);
        let health = crate::document_health::recompute_document_health(&loaded);
        assert_eq!(
            health.element_status(SceneElement::Line(1)),
            crate::document_health::HealthStatus::Unstable
        );

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_document_default_units() {
        let dir = std::env::temp_dir();
        let path = dir.join("bearcad_document_units_test.bearcad");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        doc.default_length_unit = LengthUnit::In;
        doc.default_angle_unit = AngleUnit::Rad;

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.default_length_unit, LengthUnit::In);
        assert_eq!(loaded.default_angle_unit, AngleUnit::Rad);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn legacy_files_without_unit_meta_keys_fall_back_to_mm_and_deg() {
        let dir = std::env::temp_dir();
        let path = dir.join("bearcad_legacy_units_test.bearcad");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        // Save a document, then delete the unit meta keys to simulate a pre-#52 file.
        let doc = Document::default();
        save(&path, &doc).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "DELETE FROM meta WHERE key IN (?1, ?2)",
                rusqlite::params![DEFAULT_LENGTH_UNIT_META_KEY, DEFAULT_ANGLE_UNIT_META_KEY],
            )
            .unwrap();
        }

        let loaded = open(&path).unwrap();
        assert_eq!(loaded.default_length_unit, LengthUnit::Mm);
        assert_eq!(loaded.default_angle_unit, AngleUnit::Deg);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn round_trips_sketch_unit_override_and_inherit() {
        let dir = std::env::temp_dir();
        let path = dir.join("bearcad_sketch_units_test.bearcad");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        let overridden = plane_sketch(&mut doc);
        doc.sketches[overridden].length_unit = Some(LengthUnit::Cm);
        doc.sketches[overridden].angle_unit = Some(AngleUnit::Rad);
        let inheriting = plane_sketch(&mut doc);
        assert_eq!(doc.sketches[inheriting].length_unit, None);

        save(&path, &doc).unwrap();
        let loaded = open(&path).unwrap();
        assert_eq!(loaded.sketches[overridden].length_unit, Some(LengthUnit::Cm));
        assert_eq!(loaded.sketches[overridden].angle_unit, Some(AngleUnit::Rad));
        assert_eq!(loaded.sketches[inheriting].length_unit, None);
        assert_eq!(loaded.sketches[inheriting].angle_unit, None);

        std::fs::remove_file(&path).unwrap();
    }
}
