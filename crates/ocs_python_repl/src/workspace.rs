//! Temp workspace for the Python editor and REPL.

use std::io;
use std::path::PathBuf;

/// Create a temp workspace folder and write a starter `main.py` plus editor
/// configs. Returns the workspace path.
pub fn create() -> io::Result<PathBuf> {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "ocs_python_repl_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    std::fs::create_dir_all(&dir)?;

    let main_py = include_str!("../assets/main.py");
    std::fs::write(dir.join("main.py"), main_py)?;

    // Type stubs so editors can resolve `import ocs` for static analysis.
    // The actual runtime module is the compiled ocs_acadifc extension loaded
    // by the REPL bootstrap; the .pyi file is only for type checking.
    let ocs_pyi = include_str!("../assets/ocs.pyi");
    std::fs::write(dir.join("ocs.pyi"), ocs_pyi)?;

    // Pyright config so Zed / VS Code can resolve the local ocs stub.
    std::fs::write(
        dir.join("pyrightconfig.json"),
        include_str!("../assets/pyrightconfig.json"),
    )?;

    // VS Code launch config for debugpy attach.
    let vscode_dir = dir.join(".vscode");
    std::fs::create_dir_all(&vscode_dir)?;
    std::fs::write(
        vscode_dir.join("launch.json"),
        include_str!("../assets/vscode_launch.json"),
    )?;

    Ok(dir)
}

/// Remove the workspace folder. Called when the REPL session ends.
#[allow(dead_code)]
pub fn remove(dir: &std::path::Path) -> io::Result<()> {
    std::fs::remove_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_contains_starter_files() {
        let dir = create().expect("create workspace");
        assert!(dir.join("main.py").exists());
        assert!(dir.join("ocs.pyi").exists());
        assert!(dir.join("pyrightconfig.json").exists());
        assert!(dir.join(".vscode").join("launch.json").exists());
        // ocs.py must NOT be present; the runtime module comes from the
        // ocs_acadifc extension loaded by the REPL bootstrap.
        assert!(!dir.join("ocs.py").exists());
        let _ = remove(&dir);
    }
}
