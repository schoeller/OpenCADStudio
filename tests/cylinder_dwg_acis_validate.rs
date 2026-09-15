//! Diagnosis for the CYLINDER → DWG 2018 (AC1032) faulty-save report.
//!
//! The plan (`/.kilo/plans/…cylinder-dwg-sat-validate.md`) traced the pipeline and
//! found `SatDocument::validate()` is never called on the production write path. But
//! for a *fresh* cylinder the SAB bytes are already round-trip-proven at creation, so
//! this test establishes what is actually wrong before committing to a fix:
//!
//!   1. Build the exact reported solid (centre 0,0,0, radius 1, height 2).
//!   2. Export to SAT via the host's `solid_to_sat` (the creation path).
//!   3. Validate the SAT document and re-run the *writer's* SAT→SAB conversion
//!      (`parse` → `strip_for_sab` → `SabWriter::write` → `SabReader::read`), which is
//!      what `queue_sab_entry` / `queue_sab_data` do — checking validity at each step.
//!   4. Round-trip a real DWG 2018 byte buffer and confirm the solid survives with
//!      geometry intact.

use OpenCADStudio::scene::convert::acis_export::solid_to_sat;
use OpenCADStudio::scene::model::solid_model;
use acadrust::entities::acis::{SabReader, SabWriter, SatDocument};
use acadrust::EntityType;

fn cylinder_sat() -> SatDocument {
    let body = solid_model::elliptical_cylinder_solid([0.0, 0.0, 0.0], 1.0, 1.0, 2.0)
        .expect("kernel should build a cylinder body");
    solid_to_sat(&body).expect("cylinder body should export to SAT")
}

#[test]
fn fresh_cylinder_sat_is_structurally_valid() {
    let sat = cylinder_sat();
    let errors = sat.validate();
    assert!(
        errors.is_empty(),
        "freshly exported cylinder SAT should validate, got: {errors:?}"
    );
}

/// Regression guard for the BricsCAD-rejection root cause: the kernel-exported
/// cylinder records must carry the ACIS-mandatory trailing fields, not just the
/// geometry payload. acadrust's `validate()` only checks pointer bounds, so it
/// never caught the truncation — assert the tokens directly.
#[test]
fn fresh_cylinder_sat_records_carry_mandatory_trailing_fields() {
    let sat = cylinder_sat();
    let text = sat.to_sat_string();

    assert_eq!(
        sat.header.num_bodies, 1,
        "header num_bodies should count the cylinder body"
    );

    let records_of = |kind: &str| {
        sat.records
            .iter()
            .filter(|r| r.entity_type == kind)
            .map(|r| format!("{}", r))
            .collect::<Vec<_>>()
    };

    // Helper: does a record's text form end with the expected trailing tokens?
    let has_tail = |line: &str, tail: &str| line.trim_end_matches('#').trim_end().ends_with(tail);

    for line in records_of("plane-surface") {
        assert!(has_tail(&line, "forward_v I I I I"), "plane-surface truncated: {line}");
    }
    for line in records_of("cone-surface") {
        assert!(has_tail(&line, "forward I I I I"), "cone-surface truncated: {line}");
    }
    for line in records_of("ellipse-curve") {
        assert!(has_tail(&line, "I I"), "ellipse-curve truncated: {line}");
    }
    for line in records_of("straight-curve") {
        assert!(has_tail(&line, "I I"), "straight-curve truncated: {line}");
    }
    for line in records_of("edge") {
        assert!(line.contains("unknown"), "edge missing \"unknown\" string: {line}");
    }

    // Sanity: there must be at least one of each curved record for a cylinder.
    assert!(text.contains("cone-surface"), "cylinder needs a cone-surface");
    assert!(text.contains("ellipse-curve"), "cylinder needs ellipse-curves");
}

#[test]
fn writer_sat_to_sab_conversion_preserves_validity() {
    // Mirror exactly what `queue_sab_entry` (DWG) and `queue_sab_data` (DXF) do when
    // handed SAT text: re-parse, strip for SAB, serialize, read back.
    let sat_text = cylinder_sat().to_sat_string();

    let mut reparsed = SatDocument::parse(&sat_text).expect("stored SAT should re-parse");
    let before = reparsed.validate();
    assert!(before.is_empty(), "re-parsed SAT invalid before strip: {before:?}");

    reparsed.strip_for_sab();
    let after_strip = reparsed.validate();
    assert!(
        after_strip.is_empty(),
        "strip_for_sab introduced invalid pointers: {after_strip:?}"
    );

    let sab = SabWriter::write(&reparsed);
    assert!(!sab.is_empty(), "SAB output should be non-empty");

    let readback = SabReader::read(&sab).expect("emitted SAB should re-read");
    let readback_errors = readback.validate();
    assert!(
        readback_errors.is_empty(),
        "SAB round-trip produced invalid document: {readback_errors:?}"
    );

    // The geometry must still lift back into exactly one valid kernel body.
    let (bodies, loss) = cadkernel::acis::lift(&readback);
    assert!(loss.is_empty(), "lift loss after SAB round-trip: {loss:?}");
    assert_eq!(bodies.len(), 1, "expected exactly one body back");
    let body_errors = bodies[0].validate();
    assert!(
        body_errors.is_empty(),
        "lifted cylinder body invalid after SAB round-trip: {body_errors:?}"
    );
}

#[test]
fn cylinder_survives_dwg_2018_roundtrip() {
    let mut scene = OpenCADStudio::scene::Scene::new();
    let body = solid_model::elliptical_cylinder_solid([0.0, 0.0, 0.0], 1.0, 1.0, 2.0)
        .expect("kernel should build a cylinder body");
    let sat = solid_to_sat(&body).expect("cylinder body should export to SAT");

    let mut solid = acadrust::entities::Solid3D::new();
    solid.set_sat_document(&sat);
    solid.wires = solid_model::edge_wires(&body);
    scene.add_entity(EntityType::Solid3D(solid));

    let bytes = OpenCADStudio::io::save_to_bytes(
        &scene.document,
        "dwg",
        acadrust::DxfVersion::AC1032, // DWG 2018
    )
    .expect("cylinder document should save to DWG 2018");
    assert!(!bytes.is_empty(), "DWG output should be non-empty");

    let reloaded = OpenCADStudio::io::load_bytes("cylinder-roundtrip.dwg", bytes)
        .expect("cylinder document should reload");

    let solid = reloaded
        .entities()
        .find_map(|e| match e {
            EntityType::Solid3D(s) => Some(s),
            _ => None,
        })
        .expect("the solid should round-trip through DWG 2018");

    assert!(
        solid.acis_data.has_data(),
        "reloaded solid should carry ACIS geometry"
    );

    // The reloaded geometry must parse and lift to a valid body.
    let doc = if solid.acis_data.is_binary {
        SabReader::read(&solid.acis_data.sab_data).expect("SAB should read")
    } else {
        SatDocument::parse(&solid.acis_data.sat_data).expect("SAT should parse")
    };
    let errors = doc.validate();
    assert!(errors.is_empty(), "reloaded ACIS doc invalid: {errors:?}");
    let (bodies, loss) = cadkernel::acis::lift(&doc);
    assert!(loss.is_empty(), "lift loss on reloaded solid: {loss:?}");
    assert_eq!(bodies.len(), 1, "expected one body after reload");
    assert!(
        bodies[0].validate().is_empty(),
        "reloaded cylinder body should be valid"
    );
}
