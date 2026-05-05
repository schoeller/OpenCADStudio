// I/O module — open, save, and export CAD documents.
//
// All file reading/writing goes through acadrust.
// Default save format: DWG (AC1032 / R2018+).

pub mod obj;
pub mod pdf_export;
pub mod plot_style;
pub mod print_to_printer;
pub mod step;
pub mod stl;
pub mod xref;

use acadrust::entities::{Dimension, EntityType};
use acadrust::io::dwg::DwgReader;
use acadrust::{CadDocument, DwgWriter, DxfReader, DxfWriter};
use std::path::{Path, PathBuf};

// ── Open ──────────────────────────────────────────────────────────────────

/// Show a file-open dialog and load the selected DWG or DXF file.
/// Returns `(filename, path, document)` or an error string.
pub async fn pick_and_open() -> Result<(String, PathBuf, CadDocument), String> {
    let handle = rfd::AsyncFileDialog::new()
        .set_title("Open CAD file")
        .add_filter("CAD Files", &["dwg", "dxf", "DWG", "DXF"])
        .add_filter("DWG Files", &["dwg", "DWG"])
        .add_filter("DXF Files", &["dxf", "DXF"])
        .add_filter("All Files", &["*"])
        .pick_file()
        .await;

    let handle = match handle {
        Some(h) => h,
        None => return Err("Cancelled".into()),
    };

    let path = handle.path().to_path_buf();
    open_path(path).await
}

/// Load a CAD file from a known path (used by recent files).
pub async fn open_path(path: PathBuf) -> Result<(String, PathBuf, CadDocument), String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());
    let doc = load_file(&path)?;
    Ok((name, path, doc))
}

/// Load a DWG or DXF file directly from a path (auto-detect by extension).
pub fn load_file(path: &Path) -> Result<CadDocument, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "dwg" => {
            let mut doc = DwgReader::from_file(path)
                .map_err(|e| e.to_string())?
                .read()
                .map_err(|e| e.to_string())?;
            fix_viewport_status_flags(&mut doc);
            Ok(doc)
        }
        "dxf" => {
            let mut doc = DxfReader::from_file(path)
                .map_err(|e| e.to_string())?
                .read()
                .map_err(|e| e.to_string())?;
            fix_dxf_dimension_rotations(&mut doc);
            fix_viewport_status_flags(&mut doc);
            Ok(doc)
        }
        _ => Err(format!("Unsupported file format: .{ext}")),
    }
}

// ── Directory listing (used by the custom Save As dialog) ────────────────

/// Read `dir` and return sorted entries: `(display_name, is_dir, full_path)`.
/// Directories come first, then files.  Hidden entries (`.`) are skipped.
pub fn read_dir_entries(dir: &std::path::Path) -> Vec<(String, bool, PathBuf)> {
    let mut dirs: Vec<(String, bool, PathBuf)> = Vec::new();
    let mut files: Vec<(String, bool, PathBuf)> = Vec::new();

    if let Some(parent) = dir.parent() {
        dirs.push(("..".to_string(), true, parent.to_path_buf()));
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') { continue; }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                dirs.push((name, true, entry.path()));
            } else {
                files.push((name, false, entry.path()));
            }
        }
    }
    dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    dirs.extend(files);
    dirs
}

// ── Save ──────────────────────────────────────────────────────────────────

/// Parse a format string like "DWG 2013" or "DXF 2007" into
/// `(extension, DxfVersion)`.  Falls back to ("dwg", AC1032) for unknown strings.
pub fn parse_save_format(format: &str) -> (&'static str, acadrust::DxfVersion) {
    use acadrust::DxfVersion;
    let f = format.to_ascii_uppercase();
    let is_dxf = f.starts_with("DXF");
    let ext = if is_dxf { "dxf" } else { "dwg" };
    let version = if f.contains("2013") { DxfVersion::AC1027 }
        else if f.contains("2010")      { DxfVersion::AC1024 }
        else if f.contains("2007")      { DxfVersion::AC1021 }
        else if f.contains("2004")      { DxfVersion::AC1018 }
        else if f.contains("2000")      { DxfVersion::AC1015 }
        else if f.contains("R14")       { DxfVersion::AC1014 }
        else                            { DxfVersion::AC1032 }; // 2018
    (ext, version)
}

// ── Plot Style Table ──────────────────────────────────────────────────────

/// Show a file-open dialog and load the selected CTB or STB file.
pub async fn pick_plot_style() -> Option<plot_style::PlotStyleTable> {
    let handle = rfd::AsyncFileDialog::new()
        .set_title("Load Plot Style Table")
        .add_filter("Plot Style Tables", &["ctb", "stb", "CTB", "STB"])
        .add_filter("CTB Files", &["ctb", "CTB"])
        .add_filter("STB Files", &["stb", "STB"])
        .add_filter("All Files", &["*"])
        .pick_file()
        .await?;
    plot_style::PlotStyleTable::load(handle.path()).ok()
}

// ── Image file picker ─────────────────────────────────────────────────────

/// Show a file-open dialog for raster images and decode the selected file.
/// Returns `(path, pixel_width, pixel_height)` or an error string.
pub async fn pick_image_file() -> Result<(PathBuf, u32, u32), String> {
    let handle = rfd::AsyncFileDialog::new()
        .set_title("Select Image File")
        .add_filter("Images", &["png", "jpg", "jpeg", "bmp", "tiff", "tif"])
        .add_filter("PNG", &["png"])
        .add_filter("JPEG", &["jpg", "jpeg"])
        .add_filter("All Files", &["*"])
        .pick_file()
        .await
        .ok_or_else(|| "Cancelled".to_string())?;
    let path = handle.path().to_path_buf();
    let img = image::open(&path).map_err(|e| e.to_string())?;
    let (w, h) = image::GenericImageView::dimensions(&img);
    Ok((path, w, h))
}

/// Save `doc` to `path` with the given DXF version, overriding `doc.version`.
/// Format is auto-detected from the extension (dwg / dxf).
pub fn save_as_version(
    doc: &CadDocument,
    path: &Path,
    version: acadrust::DxfVersion,
) -> Result<(), String> {
    let mut doc = doc.clone();
    doc.version = version;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "dxf" => DxfWriter::new(&doc).write_to_file(path).map_err(|e| e.to_string()),
        _     => DwgWriter::write_to_file(path, &doc).map_err(|e| e.to_string()),
    }
}

/// Save using the document's existing version.
pub fn save(doc: &CadDocument, path: &Path) -> Result<(), String> {
    save_as_version(doc, path, doc.version)
}


// ── Post-load fixups ──────────────────────────────────────────────────────

/// acadrust's ViewportStatusFlags::from_bits() maps bit 0 → is_on and bit 15 → locked,
/// but the real DXF/DWG spec uses bit 15 (0x8000) → viewport on and bit 14 (0x4000) → locked.
/// Files from AutoCAD and other tools always set bit 15 for active viewports, leaving bit 0
/// clear, so acadrust reads every such viewport as off.  Correct that here after loading.
fn fix_viewport_status_flags(doc: &mut CadDocument) {
    for entity in doc.entities_mut() {
        if let EntityType::Viewport(vp) = entity {
            let bits = vp.status.to_bits();
            // If bit 0 is not set but bit 15 is, this is an external-format viewport:
            // treat bit 15 as "on" and bit 14 as "locked".
            if (bits & 0x0001) == 0 && (bits & 0x8000) != 0 {
                vp.status.is_on = true;
                vp.status.locked = (bits & 0x4000) != 0;
            }
        }
    }
}

/// The acadrust DXF reader stores several rotation fields directly from DXF
/// group code 50 in degrees, while DWG and our own creation code store radians.
/// Apply to_radians() on load so tessellation can call cos/sin uniformly.
fn fix_dxf_dimension_rotations(doc: &mut CadDocument) {
    for entity in doc.entities_mut() {
        match entity {
            EntityType::Dimension(Dimension::Linear(d)) => {
                d.rotation = d.rotation.to_radians();
            }
            EntityType::AttributeDefinition(a) => {
                a.rotation = a.rotation.to_radians();
            }
            EntityType::AttributeEntity(a) => {
                a.rotation = a.rotation.to_radians();
            }
            EntityType::Shape(s) => {
                s.rotation = s.rotation.to_radians();
            }
            _ => {}
        }
    }
}
