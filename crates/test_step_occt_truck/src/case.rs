//! STEP test case definitions.

use std::path::{Path, PathBuf};
use crate::metrics::golden_path;

/// A single STEP-based test case.
///
/// The case is intentionally minimal: just a display name and a path to the
/// STEP file that the OCS kernel reads. For surface models that were
/// converted from geometrically-bounded STEP files, `original_path` points to
/// the original surface file so the cadrum reference can read it directly.
#[derive(Clone, Debug)]
pub struct StepCase {
    pub name: String,
    pub step_path: PathBuf,
    pub note: String,
    pub original_path: Option<PathBuf>,
}

impl StepCase {
    /// Construct a case from a STEP file under the crate's `input_brep/` directory.
    pub fn from_input_file<P: AsRef<Path>>(path: P) -> Option<Self> {
        let path = path.as_ref();
        let stem = path.file_stem()?.to_string_lossy().to_string();
        let name = format!("step_json/{stem}");
        let note = format!("STEP file: {}", path.file_name()?.to_string_lossy());
        Some(Self {
            name,
            step_path: path.to_path_buf(),
            note,
            original_path: None,
        })
    }

    /// Construct a case from a converted surface STEP file under
    /// `input_brep_surface/`, preserving the path to the original surface file.
    pub fn from_converted_surface_file<P: AsRef<Path>>(path: P, original: P) -> Option<Self> {
        let mut case = Self::from_input_file(path)?;
        let stem = case
            .step_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        case.name = format!("surface/{stem}");
        case.note = format!(
            "Converted planar B-rep: {}; original surface: {}",
            case.step_path.file_name()?.to_string_lossy(),
            original.as_ref().file_name()?.to_string_lossy()
        );
        case.original_path = Some(original.as_ref().to_path_buf());
        Some(case)
    }

    /// Returns true if this case is a converted surface model (not a native B-rep).
    pub fn is_surface(&self) -> bool {
        self.original_path.is_some()
    }

    /// Golden file path for this case, rooted at `data/step_json/`.
    pub fn golden_path(&self) -> PathBuf {
        golden_path(&self.name)
    }
}

/// Enumerate all `.stp` files in `input_brep/`.
pub fn step_cases() -> Vec<StepCase> {
    enumerate_input_dir("input_brep")
}

/// Enumerate all converted surface B-rep files in `input_brep_surface/`.
///
/// Each returned case remembers the path to the original geometrically-bounded
/// surface file in `input/`.
pub fn surface_step_cases() -> Vec<StepCase> {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let input_dir = manifest.join("input_brep_surface");
    let original_dir = manifest.join("input");

    let mut cases = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&input_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext.eq_ignore_ascii_case("stp") || ext.eq_ignore_ascii_case("step") {
                    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string());
                    if let Some(stem) = stem {
                        let original = original_dir.join(&stem).with_extension("stp");
                        if original.exists() {
                            if let Some(case) =
                                StepCase::from_converted_surface_file(&path, &original)
                            {
                                cases.push(case);
                            }
                        }
                    }
                }
            }
        }
    }
    cases.sort_by(|a, b| a.name.cmp(&b.name));
    cases
}

/// Enumerate all cases: native B-rep solids plus converted surface models.
pub fn all_cases() -> Vec<StepCase> {
    let mut cases = step_cases();
    cases.extend(surface_step_cases());
    cases
}

fn enumerate_input_dir(dir_name: &str) -> Vec<StepCase> {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let input_dir = manifest.join(dir_name);

    let mut cases = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&input_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext.eq_ignore_ascii_case("stp") || ext.eq_ignore_ascii_case("step") {
                    if let Some(case) = StepCase::from_input_file(&path) {
                        cases.push(case);
                    }
                }
            }
        }
    }
    cases.sort_by(|a, b| a.name.cmp(&b.name));
    cases
}
