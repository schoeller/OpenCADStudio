// print_to_printer — send the current layout to the system printer.
//
// Strategy:
//   1. Render the drawing to a temporary PDF (reusing the PDF export pipeline).
//   2. Send that PDF to the system printer with `lp` (Linux/macOS) or
//      `ShellExecute PRINT` (Windows).
//
// The function is async so the UI remains responsive while the job is queued.

use crate::io::pdf_export;
use crate::io::plot_style::PlotStyleTable;
use crate::scene::model::hatch_model::HatchModel;
use crate::scene::WireModel;

/// Render `wires` (plus hatch / wipeout fills) to a temp PDF and dispatch it
/// to the default system printer.
///
/// Returns `Ok(printer_name)` on success or `Err(message)` on failure.
pub async fn print_wires(
    wires: Vec<WireModel>,
    hatches: Vec<HatchModel>,
    wipeouts: Vec<HatchModel>,
    paper_w: f64,
    paper_h: f64,
    offset_x: f32,
    offset_y: f32,
    rotation_deg: i32,
    plot_style: Option<PlotStyleTable>,
) -> Result<String, String> {
    // ── 1. Write to a named temp file ─────────────────────────────────────
    let tmp_path = std::env::temp_dir().join("open_cad_studio_print.pdf");
    pdf_export::export_pdf(
        &wires,
        &hatches,
        &wipeouts,
        paper_w,
        paper_h,
        offset_x,
        offset_y,
        rotation_deg,
        &tmp_path,
        plot_style.as_ref(),
    )?;

    // ── 2. Dispatch to system printer ─────────────────────────────────────
    dispatch_to_printer(&tmp_path)
}

/// Platform-specific dispatch of a PDF path to the system printer.
fn dispatch_to_printer(path: &std::path::Path) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        // Windows: ShellExecute with "print" verb.
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let verb: Vec<u16> = OsStr::new("print\0").encode_wide().collect();
        let result = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                path_wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
            ) as usize
        };
        if result > 32 {
            Ok("default printer".to_string())
        } else {
            Err(format!("ShellExecute PRINT failed (code {result})"))
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Linux / macOS: prefer `lp`, fall back to `lpr`.
        let path_str = path.to_string_lossy();

        // Try `lp` first (CUPS).
        let lp = std::process::Command::new("lp")
            .arg("--")
            .arg(path_str.as_ref())
            .output();

        match lp {
            Ok(out) if out.status.success() => {
                // `lp` prints the job ID on stdout, e.g. "request id is lp-42 (1 file(s))"
                let msg = String::from_utf8_lossy(&out.stdout);
                let printer = msg
                    .split_whitespace()
                    .find(|w| w.contains('-'))
                    .unwrap_or("default")
                    .to_string();
                return Ok(printer);
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr).into_owned();
                // Fall through to lpr.
                if !err.is_empty() {
                    // Try lpr as alternative.
                }
            }
            Err(_) => {
                // lp not found — try lpr.
            }
        }

        // Fall back to `lpr`.
        let lpr = std::process::Command::new("lpr")
            .arg(path_str.as_ref())
            .output()
            .map_err(|e| format!("Could not launch lp or lpr: {e}"))?;

        if lpr.status.success() {
            Ok("default printer".to_string())
        } else {
            let err = String::from_utf8_lossy(&lpr.stderr).into_owned();
            Err(format!("lpr error: {err}"))
        }
    }
}
