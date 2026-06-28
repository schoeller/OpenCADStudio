use super::*;

impl OpenCADStudio {
    pub(super) fn dispatch_fileops(&mut self, cmd: &str, i: usize) -> Option<Task<Message>> {
        match cmd {
            "NEW" => return Some(Task::done(Message::TabNew)),
            "OPEN" => return Some(Task::done(Message::OpenFile)),
            "SAVE" | "QSAVE" => return Some(Task::done(Message::SaveFile)),
            "SAVEAS" => return Some(Task::done(Message::SaveAs)),
            // UNDO <n> — step back n operations at once; bare UNDO / U is one step.
            cmd if cmd.starts_with("UNDO ") => {
                let arg = cmd["UNDO ".len()..].trim();
                match arg.parse::<usize>() {
                    Ok(0) => return Some(Task::none()),
                    Ok(n) => return Some(Task::done(Message::UndoMany(n))),
                    Err(_) => {
                        self.command_line
                            .push_error("Usage: UNDO [number of steps]");
                        return Some(Task::none());
                    }
                }
            }
            "UNDO" | "U" => return Some(Task::done(Message::Undo)),
            "REDO" => return Some(Task::done(Message::Redo)),
            "CLEAR" | "CLR" => return Some(Task::done(Message::ClearScene)),
            "WIREFRAME" | "VW" => return Some(Task::done(Message::SetWireframe(true))),
            "SOLID" | "VS" => return Some(Task::done(Message::SetWireframe(false))),
            "EXIT" | "QUIT" => {
                // Funnel through the OS close path so the unsaved-changes
                // dialog runs before `iced::exit()`. Falls back to a hard
                // exit if there's no main window registered yet.
                if let Some(id) = self.main_window {
                    return Some(Task::done(Message::WindowCloseRequested(id)));
                }
                return Some(iced::exit());
            }

            // ── Frame-budget HUD (Phase 5.3) ───────────────────────────────
            // Toggle the per-rebuild wire-tessellation readout overlay.
            "PERF" => {
                self.perf_hud = !self.perf_hud;
                self.command_line.push_info(if self.perf_hud {
                    "PERF HUD on — shows last wire re-tessellation cost"
                } else {
                    "PERF HUD off"
                });
                return Some(Task::none());
            }

            // ── Background color ───────────────────────────────────────────
            // Usage:  BACKGROUND <r> <g> <b>      (0–255 each)
            //         BACKGROUND WHITE|BLACK|GRAY|DARKGRAY|LTGRAY   (preset)
            //         BACKGROUND RESET            (restore default)
            // The chosen colour is also stored as the persisted default
            // (`default_bg_color` / `default_paper_bg_color`) so it survives
            // restarts and applies to new drawings (#188).
            cmd if cmd == "BACKGROUND" || cmd.starts_with("BACKGROUND ") => {
                let args = cmd.split_whitespace().skip(1).collect::<Vec<_>>();
                let is_paper = self.tabs[i].scene.current_layout != "Model";
                if args
                    .first()
                    .map(|s| s.eq_ignore_ascii_case("RESET"))
                    .unwrap_or(false)
                {
                    if is_paper {
                        self.tabs[i].paper_bg_color = None;
                        self.tabs[i].scene.paper_bg_color = [1.0, 1.0, 1.0, 1.0];
                        self.default_paper_bg_color = None;
                    } else {
                        self.tabs[i].bg_color = None;
                        self.tabs[i].scene.bg_color = [0.11, 0.11, 0.11, 1.0];
                        self.default_bg_color = None;
                    }
                    // Wire colour adaptation (`adapt_to_bg`) reads the bg
                    // at tessellation time, so the cached wires need to
                    // refresh — otherwise a light→dark bg flip leaves
                    // black lines invisible against the new bg. Meshes
                    // bake colour into per-vertex GPU buffers at upload
                    // time; `recolor_meshes` rewrites the CPU side so
                    // the next epoch-driven re-upload picks up the new
                    // colour.
                    self.tabs[i].scene.recolor_meshes();
                    self.tabs[i].scene.bump_geometry();
                    self.command_line
                        .push_output("Background reset to default.");
                } else if let Some(rgba) = parse_background_color(&args) {
                    if is_paper {
                        self.tabs[i].paper_bg_color = Some(rgba);
                        self.tabs[i].scene.paper_bg_color = rgba;
                        self.default_paper_bg_color = Some(rgba);
                    } else {
                        self.tabs[i].bg_color = Some(rgba);
                        self.tabs[i].scene.bg_color = rgba;
                        self.default_bg_color = Some(rgba);
                    }
                    self.tabs[i].scene.recolor_meshes();
                    self.tabs[i].scene.bump_geometry();
                    let [r, g, b, _] = rgba;
                    self.command_line.push_output(&format!(
                        "Background: rgb({}, {}, {})",
                        (r * 255.0).round() as u8,
                        (g * 255.0).round() as u8,
                        (b * 255.0).round() as u8
                    ));
                    // Persisted centrally after this message via
                    // `persist_settings_if_changed()`.
                } else {
                    self.command_line.push_info(
                        "Usage: BACKGROUND <r> <g> <b> (0–255) | WHITE|BLACK|GRAY|DARKGRAY|LTGRAY | RESET",
                    );
                }
            }
            "ORTHO" => return Some(Task::done(Message::SetProjection(true))),
            "PERSP" => return Some(Task::done(Message::SetProjection(false))),
            "LAYERS" | "LA" => return Some(Task::done(Message::ToggleLayers)),

            // SCRIPT <path> — run a command script: each non-blank, non-comment
            // line is fed through the same command path the `--script` startup
            // flag uses, so the behaviour matches headless automation exactly.
            cmd if cmd == "SCRIPT"
                || cmd == "SCR"
                || cmd.starts_with("SCRIPT ")
                || cmd.starts_with("SCR ") =>
            {
                let path = cmd.split_once(' ').map(|(_, r)| r.trim().to_string());
                match path {
                    Some(p) if !p.is_empty() => match std::fs::read_to_string(&p) {
                        Ok(text) => {
                            let cmds: Vec<Task<Message>> = text
                                .lines()
                                .map(str::trim)
                                .filter(|l| {
                                    !l.is_empty() && !l.starts_with('#') && !l.starts_with(';')
                                })
                                .map(|l| Task::done(Message::Command(l.to_string())))
                                .collect();
                            self.command_line.push_output(&format!(
                                "SCRIPT: running {} command(s) from {p}.",
                                cmds.len()
                            ));
                            return Some(Task::batch(cmds));
                        }
                        Err(e) => {
                            self.command_line
                                .push_error(&format!("SCRIPT: cannot read {p}: {e}"));
                        }
                    },
                    _ => {
                        self.command_line
                            .push_info("Usage: SCRIPT <path to .scr file>");
                    }
                }
            }

            _ => return None,
        }
        Some(self.finish_dispatch(cmd))
    }
}

/// Parse the argument list of the `BACKGROUND` command into an `[r,g,b,a]`
/// colour (channels 0.0–1.0, `a` always 1.0). Accepts:
///   * three whitespace-separated 0–255 values: `255 255 255`
///   * a named preset: WHITE / BLACK / GRAY|GREY / DARKGRAY|DARKGREY / LTGRAY
/// Returns `None` if the arguments don't match either form.
fn parse_background_color(args: &[&str]) -> Option<[f32; 4]> {
    let to_rgba = |[r, g, b]: [u8; 3]| {
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
    };
    // Single token: a named preset.
    if args.len() == 1 {
        let preset = match args[0].to_ascii_uppercase().as_str() {
            "WHITE" => [255, 255, 255],
            "BLACK" => [0, 0, 0],
            "GRAY" | "GREY" => [128, 128, 128],
            "DARKGRAY" | "DARKGREY" | "DKGRAY" => [64, 64, 64],
            "LTGRAY" | "LIGHTGRAY" | "LIGHTGREY" => [192, 192, 192],
            _ => return None,
        };
        return Some(to_rgba(preset));
    }
    // Three separate tokens: `r g b`.
    if args.len() >= 3 {
        let r = args[0].parse::<u8>().ok()?;
        let g = args[1].parse::<u8>().ok()?;
        let b = args[2].parse::<u8>().ok()?;
        return Some(to_rgba([r, g, b]));
    }
    None
}
