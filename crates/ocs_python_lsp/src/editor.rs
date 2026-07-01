//! External editor detection and launch.

use std::path::Path;
use std::process::{Command, Stdio};

/// Known editor with a human-readable name and launch command.
struct Editor {
    name: &'static str,
    program: &'static str,
    #[cfg(windows)]
    windows_program: Option<&'static str>,
}

const EDITORS: &[Editor] = &[
    Editor {
        name: "Lapce",
        program: "lapce",
        #[cfg(windows)]
        windows_program: None,
    },
    Editor {
        name: "Zed",
        program: "zed",
        #[cfg(windows)]
        windows_program: None,
    },
    Editor {
        name: "VS Code",
        program: "code",
        #[cfg(windows)]
        windows_program: Some("code.cmd"),
    },
];

fn program_name(editor: &Editor) -> &'static str {
    #[cfg(windows)]
    if let Some(win) = editor.windows_program {
        return win;
    }
    editor.program
}

/// Detect the first available editor in priority order.
pub fn detect_editor() -> Option<(&'static str, Command)> {
    for editor in EDITORS {
        let name = program_name(editor);
        if is_available(name) {
            let cmd = Command::new(name);
            return Some((editor.name, cmd));
        }
    }
    None
}

fn is_available(name: &str) -> bool {
    let output = Command::new(name).arg("--version").output();
    matches!(output, Ok(out) if out.status.success())
}

/// Launch the first available editor with `workspace`.
pub fn launch_editor(workspace: &Path) -> Result<&'static str, String> {
    let Some((name, mut cmd)) = detect_editor() else {
        return Err("No external editor found; install Lapce, Zed, or VS Code.".to_string());
    };

    cmd.arg(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        const DETACHED_PROCESS: u32 = 0x00000008;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    cmd.spawn()
        .map_err(|e| format!("failed to launch {name}: {e}"))?;
    Ok(name)
}
