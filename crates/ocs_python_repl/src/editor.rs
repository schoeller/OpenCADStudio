//! External editor launch.

use std::io;
use std::path::Path;
use std::process::Command;

/// Try to launch a supported editor with the workspace folder, and also pass
/// `main.py` so editors that support it (Zed, VS Code) open the file directly.
/// Supported editors (in preference order): Zed, Gram, VS Code, Lite XL, Lapce.
pub fn launch(workspace: &Path) -> io::Result<()> {
    let main_py = workspace.join("main.py");

    // Zed: open the workspace *and* main.py so the workspace config
    // (pyrightconfig.json / ocs.pyi) is loaded while main.py is focused.
    if let Some(exe) = find_exe("zed") {
        let status = Command::new(exe).arg(workspace).arg(&main_py).status()?;
        if status.success() {
            return Ok(());
        }
        eprintln!("Zed exited with {status}; trying next editor");
    }

    // Gram: open the workspace in a new window.
    if let Some(exe) = find_exe("gram") {
        let status = Command::new(exe).arg("-n").arg(workspace).status()?;
        if status.success() {
            return Ok(());
        }
        eprintln!("Gram exited with {status}; trying next editor");
    }

    // VS Code: open the workspace and main.py.
    if let Some(exe) = find_exe("code") {
        let status = Command::new(exe).arg(workspace).arg(&main_py).status()?;
        if status.success() {
            return Ok(());
        }
        eprintln!("VS Code exited with {status}; trying next editor");
    }

    // Lite XL: open the workspace.
    if let Some(exe) = find_exe("lite-xl") {
        let status = Command::new(exe).arg(workspace).status()?;
        if status.success() {
            return Ok(());
        }
        eprintln!("Lite XL exited with {status}; trying next editor");
    }

    // Lapce: open the workspace.
    if let Some(exe) = find_exe("lapce") {
        let status = Command::new(exe).arg(workspace).status()?;
        if status.success() {
            return Ok(());
        }
        eprintln!("Lapce exited with {status}; trying next editor");
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no supported editor found (Zed, Gram, VS Code, Lite XL, Lapce)",
    ))
}

fn find_exe(name: &str) -> Option<std::path::PathBuf> {
    let output = Command::new(if cfg!(windows) { "where" } else { "which" })
        .arg(name)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let line = output
        .stdout
        .split(|&b| b == b'\n' || b == b'\r')
        .next()
        .filter(|l| !l.is_empty())?;
    let s = std::str::from_utf8(line).ok()?;
    Some(std::path::PathBuf::from(s.trim()))
}
