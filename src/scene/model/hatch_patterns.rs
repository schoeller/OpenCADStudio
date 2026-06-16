// OpenCADStudio hatch pattern catalog — built from `assets/patterns/OpenCADStudio.pat`.
//
// Each `PatternEntry` wraps a parsed PAT pattern with:
//   - `gpu`       — `HatchPattern::Pattern(families)` for the shader
//   - `pat_lines` — exact PAT line definitions used for DXF export

use std::sync::OnceLock;

use crate::patterns::PatLineDef;
use crate::scene::model::hatch_model::{HatchPattern, PatFamily};
use acadrust::entities::{HatchPattern as DxfPattern, HatchPatternLine};
use acadrust::types::Vector2;

// ── Public types ──────────────────────────────────────────────────────────

pub struct PatternEntry {
    pub name: String,
    pub description: String,
    /// GPU-ready pattern for the shader.
    pub gpu: HatchPattern,
    /// Exact PAT line families (used for DXF export).
    pub pat_lines: Vec<PatLineDef>,
}

// ── Catalog ───────────────────────────────────────────────────────────────

static CATALOG: OnceLock<Vec<PatternEntry>> = OnceLock::new();

pub fn catalog() -> &'static [PatternEntry] {
    CATALOG.get_or_init(build_catalog)
}

pub fn find(name: &str) -> Option<&'static PatternEntry> {
    catalog().iter().find(|e| e.name.eq_ignore_ascii_case(name))
}

pub fn names() -> Vec<String> {
    catalog().iter().map(|e| e.name.clone()).collect()
}

// ── DXF export ────────────────────────────────────────────────────────────

pub fn build_dxf_pattern(entry: &PatternEntry) -> DxfPattern {
    let mut pat = DxfPattern::new(&entry.name);
    pat.description = entry.description.clone();
    for ln in &entry.pat_lines {
        let angle_rad = (ln.angle_deg as f64).to_radians();
        pat.lines.push(HatchPatternLine {
            angle: angle_rad,
            base_point: Vector2::new(ln.x0 as f64, ln.y0 as f64),
            offset: Vector2::new(ln.dx as f64, ln.dy as f64),
            dash_lengths: ln.dashes.iter().map(|&d| d as f64).collect(),
        });
    }
    pat
}

// ── Builder ───────────────────────────────────────────────────────────────

fn build_catalog() -> Vec<PatternEntry> {
    let mut entries = vec![PatternEntry {
        name: "SOLID".into(),
        description: "Solid fill".into(),
        gpu: HatchPattern::Solid,
        pat_lines: vec![],
    }];

    for def in crate::patterns::catalog() {
        entries.push(PatternEntry {
            name: def.name.clone(),
            description: def.description.clone(),
            gpu: HatchPattern::Pattern(def.lines.iter().map(pat_line_to_family).collect()),
            pat_lines: def.lines.clone(),
        });
    }
    entries
}

fn pat_line_to_family(ln: &PatLineDef) -> PatFamily {
    PatFamily {
        angle_deg: ln.angle_deg,
        x0: ln.x0,
        y0: ln.y0,
        dx: ln.dx,
        dy: ln.dy,
        dashes: ln.dashes.clone(),
    }
}
