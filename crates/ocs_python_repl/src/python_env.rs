//! Python interpreter / PIP detection and package installation.

use std::io;
use std::path::PathBuf;
use std::process::Command;

/// Locate the Python interpreter and PIP executable. On Windows the launcher is
/// `py`; on Unix we prefer `python3`. Returns `(python, pip)`.
pub fn ensure_python() -> io::Result<(PathBuf, PathBuf)> {
    let python = find_python()?;
    let pip = find_pip(&python)?;
    Ok((python, pip))
}

fn find_python() -> io::Result<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["py", "python", "python3"]
    } else {
        &["python3", "python"]
    };
    for name in names {
        if let Ok(path) = which(name) {
            return Ok(path);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no Python interpreter found; install Python 3 and make sure it is on PATH",
    ))
}

fn find_pip(python: &std::path::Path) -> io::Result<PathBuf> {
    // Try `python -m pip` first.
    let output = Command::new(python)
        .args(["-m", "pip", "--version"])
        .output()?;
    if output.status.success() {
        // Return the same interpreter; callers will invoke `python -m pip`.
        return Ok(python.to_path_buf());
    }

    // Fall back to a standalone `pip`/`pip3` executable.
    let names: &[&str] = if cfg!(windows) {
        &["pip", "pip3"]
    } else {
        &["pip3", "pip"]
    };
    for name in names {
        if let Ok(path) = which(name) {
            return Ok(path);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "pip not found; install pip for the selected Python interpreter",
    ))
}

/// Install `package` via PIP if it is not already importable.
pub fn ensure_package(python_or_pip: &std::path::Path, package: &str) -> io::Result<()> {
    let output = Command::new(python_or_pip)
        .args(["-c", &format!("import {}", package)])
        .output()?;
    if output.status.success() {
        return Ok(());
    }

    let output = Command::new(python_or_pip)
        .args(["-m", "pip", "install", "--user", package])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("pip install {package} failed: {stderr}"),
        ));
    }
    Ok(())
}

fn which(name: &str) -> io::Result<PathBuf> {
    let output = Command::new(if cfg!(windows) { "where" } else { "which" })
        .arg(name)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "not found"));
    }
    let line = output
        .stdout
        .split(|&b| b == b'\n' || b == b'\r')
        .next()
        .filter(|l| !l.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "empty output"))?;
    let s = std::str::from_utf8(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(PathBuf::from(s.trim()))
}
