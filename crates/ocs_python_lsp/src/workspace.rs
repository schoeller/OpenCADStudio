//! Temporary workspace generation for each `PYTHONEDIT` invocation.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

const BRIDGE_PY: &str = include_str!("../assets/ocs_lsp_bridge.py");

/// Paths and metadata for one editor workspace.
pub struct Workspace {
    pub root: PathBuf,
    pub port: u16,
    pub tab: usize,
}

impl Workspace {
    /// Create a fresh temp workspace bound to `tab` and `port`.
    pub fn create(tab: usize, port: u16) -> Result<Self, String> {
        let root = std::env::temp_dir().join(format!("ocs_python_lsp_{tab}_{port}"));
        if root.exists() {
            let _ = fs::remove_dir_all(&root);
        }
        fs::create_dir_all(&root)
            .map_err(|e| format!("failed to create workspace {root:?}: {e}"))?;

        let ws = Self { root, port, tab };
        ws.write_files()?;
        Ok(ws)
    }

    fn write_files(&self) -> Result<(), String> {
        // main.py starter file.
        let main_py = self.root.join("main.py");
        fs::write(
            &main_py,
            b"# OpenCAD Studio Python LSP workspace\nimport ocs\n\nprint(ocs.doc.entities())\n",
        )
        .map_err(|e| format!("failed to write main.py: {e}"))?;

        // ocs_lsp.json tells the bridge where to connect.
        let config = self.root.join("ocs_lsp.json");
        fs::write(
            &config,
            json!({ "port": self.port, "tab": self.tab, "version": 1 }).to_string(),
        )
        .map_err(|e| format!("failed to write ocs_lsp.json: {e}"))?;

        // Python bridge script.
        let bridge = self.root.join("ocs_lsp_bridge.py");
        fs::write(&bridge, BRIDGE_PY)
            .map_err(|e| format!("failed to write ocs_lsp_bridge.py: {e}"))?;

        // Lapce settings.
        let lapce_dir = self.root.join(".lapce");
        fs::create_dir_all(&lapce_dir)
            .map_err(|e| format!("failed to create .lapce dir: {e}"))?;
        fs::write(
            lapce_dir.join("settings.toml"),
            format!(
                "[[lapce-rust.lsp]]\ncommand = [\"python\", \"{}\"]\n",
                bridge.display()
            ),
        )
        .map_err(|e| format!("failed to write Lapce settings: {e}"))?;

        // Zed settings.
        let zed_dir = self.root.join(".zed");
        fs::create_dir_all(&zed_dir)
            .map_err(|e| format!("failed to create .zed dir: {e}"))?;
        fs::write(
            zed_dir.join("settings.json"),
            json!({
                "lsp": {
                    "python": {
                        "command": ["python", bridge.to_string_lossy().to_string()],
                    }
                }
            })
            .to_string(),
        )
        .map_err(|e| format!("failed to write Zed settings: {e}"))?;

        // VS Code settings and bundled extension placeholder.
        let vscode_dir = self.root.join(".vscode");
        fs::create_dir_all(&vscode_dir)
            .map_err(|e| format!("failed to create .vscode dir: {e}"))?;
        fs::write(
            vscode_dir.join("settings.json"),
            json!({
                "python.analysis.useImportHeuristic": true,
                "python.linting.enabled": false,
            })
            .to_string(),
        )
        .map_err(|e| format!("failed to write VS Code settings: {e}"))?;

        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_creates_expected_files() {
        let ws = Workspace::create(7, 12345).expect("create workspace");
        assert!(ws.root.join("main.py").exists());
        assert!(ws.root.join("ocs_lsp.json").exists());
        assert!(ws.root.join("ocs_lsp_bridge.py").exists());
        assert!(ws.root.join(".lapce/settings.toml").exists());
        assert!(ws.root.join(".zed/settings.json").exists());
        assert!(ws.root.join(".vscode/settings.json").exists());
    }
}
