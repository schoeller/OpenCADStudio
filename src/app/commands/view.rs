use super::*;

impl OpenCADStudio {
    pub(super) fn dispatch_view(&mut self, cmd: &str, i: usize) -> Option<Task<Message>> {
        match cmd {
            "HELP" | "?" => {
                self.command_line.push_output(
                    "Draw: LINE CIRCLE ARC PLINE RECTANG(RECT) POLYGON(POLY) POINT ELLIPSE SPLINE RAY XLINE HATCH DONUT REVCLOUD WIPEOUT MLINE ATTDEF  |  \
                     Modify: MOVE COPY ROTATE SCALE MIRROR ERASE OFFSET EXTEND FILLET CHAMFER STRETCH EXPLODE TRIM BREAK JOIN LENGTHEN ALIGN PEDIT  |  \
                     Array: ARRAY ARRAYRECT ARRAYPOLAR ARRAYPATH  |  \
                     Text: TEXT MTEXT LEADER MLEADER  |  \
                     Dimension: DIMLINEAR DIMALIGNED DIMANGULAR DIMRADIUS DIMDIAMETER DIMCONTINUE DIMBASELINE  |  \
                     Annotation: TOLERANCE  |  \
                     Inquiry: DIST ID AREA LIST FIND FINDALL COUNT QSELECT  |  Draw on entity: DIVIDE MEASURE  |  \
                     Attributes: ATTEDIT ATTDISP  |  \
                     Utilities: FLATTEN LAYISO LAYUNISO  |  \
                     View: ZOOM EXTENTS ZOOM WINDOW VIEW LIST/SAVE/RESTORE/DELETE  |  \
                     Layer: LAYER LIST/NEW/ON/OFF/FREEZE/THAW/LOCK/UNLOCK/COLOR/SET  |  \
                     Viewport: MVIEW VPLAYER VPORTS MS PS DRAWORDER  |  \
                     Tables: STYLE DIMSTYLE LINETYPE UCS RENAME PURGE  |  \
                     File: NEW OPEN SAVE SAVEAS PRINT PURGE UNDO REDO"
                );
            }

            "DONATE" => {
                crate::sys::open_url("https://patreon.com/HakanSeven12");
                self.command_line.push_info("Opening Patreon page...");
            }

            "WEBVERSION" => {
                crate::sys::open_url("https://hakanseven12.github.io/OpenCADStudio/");
                self.command_line.push_info("Opening OCS Web...");
            }

            // ── DWGPROPS — print round-trip-only HeaderVariables ─────────
            // No UI dialog for these yet; the command surfaces them so
            // users can confirm the values that the parser populated and
            // the writer will round-trip on save.
            "DWGPROPS" | "DWGPROP" => {
                let i = self.active_tab;
                let h = &self.tabs[i].scene.document.header;
                let path_label = self.tabs[i]
                    .current_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(unsaved)".to_string());
                self.command_line
                    .push_output(&format!("Drawing: {}", path_label));
                self.command_line.push_output(&format!(
                    "  Created (Julian):  {:.6}",
                    h.create_date_julian
                ));
                self.command_line.push_output(&format!(
                    "  Updated (Julian):  {:.6}",
                    h.update_date_julian
                ));
                self.command_line.push_output(&format!(
                    "  Total edit time:   {:.4}",
                    h.total_editing_time
                ));
                self.command_line.push_output(&format!(
                    "  User elapsed:      {:.4}",
                    h.user_elapsed_time
                ));
                self.command_line.push_output(&format!(
                    "  Last saved by:     {}",
                    if h.last_saved_by.is_empty() {
                        "(unknown)"
                    } else {
                        &h.last_saved_by
                    }
                ));
                self.command_line.push_output(&format!(
                    "  Fingerprint GUID:  {}",
                    if h.fingerprint_guid.is_empty() {
                        "(none)"
                    } else {
                        &h.fingerprint_guid
                    }
                ));
                self.command_line.push_output(&format!(
                    "  Version GUID:      {}",
                    if h.version_guid.is_empty() {
                        "(none)"
                    } else {
                        &h.version_guid
                    }
                ));
                self.command_line
                    .push_output(&format!("  Code page:         {}", h.code_page));
                self.command_line.push_output(&format!(
                    "  Menu name:         {}",
                    if h.menu_name.is_empty() {
                        "(none)"
                    } else {
                        &h.menu_name
                    }
                ));
                self.command_line.push_output(&format!(
                    "  Hyperlink base:    {}",
                    if h.hyperlink_base.is_empty() {
                        "(none)"
                    } else {
                        &h.hyperlink_base
                    }
                ));
                self.command_line.push_output(&format!(
                    "  Project name:      {}",
                    if h.project_name.is_empty() {
                        "(none)"
                    } else {
                        &h.project_name
                    }
                ));
                self.command_line.push_output(&format!(
                    "  Stylesheet:        {}",
                    if h.stylesheet.is_empty() {
                        "(none)"
                    } else {
                        &h.stylesheet
                    }
                ));
                self.command_line.push_output(&format!(
                    "  Required versions: {:#018x}",
                    h.required_versions
                ));
                self.command_line.push_output(&format!(
                    "  Measurement:       {} ({})",
                    h.measurement,
                    if h.measurement == 1 { "Metric" } else { "Imperial" }
                ));
                self.command_line.push_output(&format!(
                    "  Proxy graphics:    {}",
                    h.proxy_graphics
                ));
                self.command_line
                    .push_output(&format!("  Tree depth:        {}", h.tree_depth));
                self.command_line.push_output(&format!(
                    "  User vars (int):   {} {} {} {} {}",
                    h.user_int1, h.user_int2, h.user_int3, h.user_int4, h.user_int5
                ));
                self.command_line.push_output(&format!(
                    "  User vars (real):  {:.6} {:.6} {:.6} {:.6} {:.6}",
                    h.user_real1, h.user_real2, h.user_real3, h.user_real4, h.user_real5
                ));
                self.command_line.push_output(&format!(
                    "  User timer:        {}",
                    if h.user_timer { "On" } else { "Off" }
                ));
            }

            // Edit a USERI1..USERI5 / USERR1..USERR5 slot. Lets the user
            // store drawing-scoped scalars (and save them through round-trip)
            // even though we don't have a LISP / DIESEL runtime yet.
            //   USERI 1 42        → header.user_int1 = 42
            //   USERR 3 1.5e-3    → header.user_real3 = 0.0015
            cmd if cmd.starts_with("USERI") || cmd.starts_with("USERR") => {
                let is_real = cmd.starts_with("USERR");
                let rest = if is_real {
                    cmd.trim_start_matches("USERR").trim()
                } else {
                    cmd.trim_start_matches("USERI").trim()
                };
                let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                let slot: Option<usize> = parts.first().and_then(|s| s.parse().ok());
                let value = parts.get(1).copied().unwrap_or("").trim();
                let i = self.active_tab;
                let h = &mut self.tabs[i].scene.document.header;
                match (slot, value, is_real) {
                    (Some(n @ 1..=5), v, true) => {
                        if let Ok(val) = v.parse::<f64>() {
                            match n {
                                1 => h.user_real1 = val,
                                2 => h.user_real2 = val,
                                3 => h.user_real3 = val,
                                4 => h.user_real4 = val,
                                _ => h.user_real5 = val,
                            }
                            self.tabs[i].dirty = true;
                            self.command_line
                                .push_output(&format!("USERR{n} = {val}"));
                        } else {
                            self.command_line
                                .push_info("Usage: USERR <1-5> <real>");
                        }
                    }
                    (Some(n @ 1..=5), v, false) => {
                        if let Ok(val) = v.parse::<i16>() {
                            match n {
                                1 => h.user_int1 = val,
                                2 => h.user_int2 = val,
                                3 => h.user_int3 = val,
                                4 => h.user_int4 = val,
                                _ => h.user_int5 = val,
                            }
                            self.tabs[i].dirty = true;
                            self.command_line
                                .push_output(&format!("USERI{n} = {val}"));
                        } else {
                            self.command_line
                                .push_info("Usage: USERI <1-5> <integer>");
                        }
                    }
                    _ => self
                        .command_line
                        .push_info("Usage: USERI <1-5> <int> | USERR <1-5> <real>"),
                }
            }

            "REPORT" => {
                // Pre-fill the GitHub issue body with version + platform so
                // reports arrive with the basics already filled in.
                let body = format!(
                    "<!-- Describe the issue and the steps to reproduce it. -->\n\n\n\
                     ---\n- Open CAD Studio: v{}\n- Platform: {}\n",
                    env!("CARGO_PKG_VERSION"),
                    crate::sys::platform_info(),
                );
                let url = format!(
                    "https://github.com/HakanSeven12/OpenCADStudio/issues/new?body={}",
                    crate::sys::percent_encode(&body)
                );
                crate::sys::open_url(&url);
                self.command_line.push_info("Opening feedback page...");
            }

            "ABOUT" => {
                return Some(Task::done(Message::AboutOpen));
            }

            "PLUGINS" | "PLUGINMANAGER" => {
                return Some(Task::done(Message::PluginManagerOpen));
            }

            "CHANGELOG" => {
                crate::sys::open_url("https://github.com/HakanSeven12/OpenCADStudio/releases");
                self.command_line.push_info("Opening release notes...");
            }

            // ── CUI / ALIASEDIT — customization entry points ───────────────
            // Command-alias and key-binding editing lives in the keyboard
            // shortcuts panel, so route the customization verbs there.
            "CUI" | "ALIASEDIT" => {
                return Some(Task::done(Message::ShortcutsPanelOpen));
            }

            // ── Keyboard Shortcuts panel ──────────────────────────────────
            cmd if cmd == "SHORTCUTS" || cmd.starts_with("SHORTCUTS ") => {
                let raw_rest = cmd.trim_start_matches("SHORTCUTS").trim();
                let parts: Vec<&str> = raw_rest.splitn(3, ' ').collect();
                let sub = parts.first().map(|s| s.to_uppercase()).unwrap_or_default();
                match sub.as_str() {
                    "" | "LIST" | "?" => {
                        return Some(Task::done(Message::ShortcutsPanelOpen));
                    }
                    "SET" | "S" => {
                        // SHORTCUTS SET <key> <command>
                        // e.g. SHORTCUTS SET CTRL+D DIST
                        let key = parts.get(1).map(|s| s.to_uppercase()).unwrap_or_default();
                        let cmd_str = parts.get(2).map(|s| s.to_uppercase()).unwrap_or_default();
                        if key.is_empty() || cmd_str.is_empty() {
                            self.command_line.push_error("Usage: SHORTCUTS SET <key> <command>  e.g. SHORTCUTS SET CTRL+D DIST");
                        } else {
                            self.shortcut_overrides.insert(key.clone(), cmd_str.clone());
                            self.command_line
                                .push_output(&format!("Shortcut set: {key} → {cmd_str}"));
                        }
                    }
                    "CLEAR" | "DELETE" | "REMOVE" => {
                        let key = parts.get(1).map(|s| s.to_uppercase()).unwrap_or_default();
                        if key.is_empty() {
                            self.command_line.push_error("Usage: SHORTCUTS CLEAR <key>");
                        } else if self.shortcut_overrides.remove(&key).is_some() {
                            self.command_line
                                .push_output(&format!("Shortcut '{key}' removed."));
                        } else {
                            self.command_line
                                .push_error(&format!("Shortcut '{key}' not found."));
                        }
                    }
                    _ => {
                        self.command_line
                            .push_info("Usage: SHORTCUTS LIST | SET <key> <cmd> | CLEAR <key>");
                    }
                }
            }

            // ── Color Scheme / Theme selector ─────────────────────────────
            cmd if cmd == "COLORSCHEME" || cmd.starts_with("COLORSCHEME ") => {
                use iced::Theme;
                let sub = cmd
                    .split_once(' ')
                    .map(|(_, r)| r.trim())
                    .unwrap_or("")
                    .to_uppercase();
                // Map name to Theme variant.
                let theme: Option<Theme> = match sub.as_str() {
                    "DARK" => Some(Theme::Dark),
                    "LIGHT" => Some(Theme::Light),
                    "DRACULA" => Some(Theme::Dracula),
                    "NORD" => Some(Theme::Nord),
                    "SOLARIZED_LIGHT" | "SOLARIZEDLIGHT" => Some(Theme::SolarizedLight),
                    "SOLARIZED_DARK" | "SOLARIZEDDARK" => Some(Theme::SolarizedDark),
                    "GRUVBOX_LIGHT" | "GRUVBOXLIGHT" => Some(Theme::GruvboxLight),
                    "GRUVBOX_DARK" | "GRUVBOXDARK" => Some(Theme::GruvboxDark),
                    "TOKYONIGHT" | "TOKYO_NIGHT" => Some(Theme::TokyoNight),
                    "TOKYONIGHTSTORM" | "TOKYO_NIGHT_STORM" => Some(Theme::TokyoNightStorm),
                    "TOKYONIGHTLIGHT" | "TOKYO_NIGHT_LIGHT" => Some(Theme::TokyoNightLight),
                    "KANAGAWAWAVE" | "KANAGAWA_WAVE" => Some(Theme::KanagawaWave),
                    "KANAGAWADRAGON" | "KANAGAWA_DRAGON" => Some(Theme::KanagawaDragon),
                    "KANAGAWALOTUS" | "KANAGAWA_LOTUS" => Some(Theme::KanagawaLotus),
                    "MOONFLY" => Some(Theme::Moonfly),
                    "NIGHTFLY" => Some(Theme::Nightfly),
                    "OXOCARBON" => Some(Theme::Oxocarbon),
                    "FERRA" => Some(Theme::Ferra),
                    "" | "LIST" | "?" => {
                        self.command_line.push_output(
                            "Available themes: DARK LIGHT DRACULA NORD SOLARIZED_LIGHT SOLARIZED_DARK \
                             GRUVBOX_LIGHT GRUVBOX_DARK TOKYONIGHT TOKYONIGHTSTORM TOKYONIGHTLIGHT \
                             KANAGAWAWAVE KANAGAWADRAGON KANAGAWALOTUS MOONFLY NIGHTFLY OXOCARBON FERRA"
                        );
                        return Some(Task::none());
                    }
                    _ => {
                        self.command_line.push_error(&format!(
                            "COLORSCHEME: unknown theme '{}'. Type COLORSCHEME LIST for options.",
                            sub
                        ));
                        return Some(Task::none());
                    }
                };
                if let Some(t) = theme {
                    let name = format!("{:?}", t);
                    self.command_line
                        .push_output(&format!("Color scheme set to '{name}'."));
                    return Some(Task::done(Message::SetTheme(t)));
                }
                return Some(Task::none());
            }

            // ── Layout Manager GUI ─────────────────────────────────────────
            "LAYOUTMANAGER" | "LAYOUTPANEL" => {
                return Some(Task::done(Message::LayoutManagerOpen));
            }

            // ── Layout / viewport ──────────────────────────────────────────
            "MVIEW" | "MV" => {
                if self.tabs[i].scene.current_layout == "Model" {
                    self.command_line
                        .push_error("MVIEW: switch to a paper space layout first.");
                } else {
                    use crate::modules::layout::mview::MviewCommand;
                    let new_cmd = MviewCommand::new();
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            // ── MSPACE / PSPACE ───────────────────────────────────────────
            "MS" | "MSPACE" => {
                return Some(Task::done(Message::MspaceCommand));
            }
            "PSPACE" => {
                return Some(Task::done(Message::PspaceCommand));
            }

            // ── Viewport arrangement shortcuts ────────────────────────────
            // Tile the model viewports into preset splits. Each delegates to the
            // matching VPORTS configuration so the Model/paper handling stays in
            // one place.
            "HORIZONTAL" => return self.dispatch_view("VPORTS 2H", i),
            "VERTICAL" => return self.dispatch_view("VPORTS 2V", i),
            "VPJOIN" => return self.dispatch_view("VPORTS SINGLE", i),
            "CASCADE" => return self.dispatch_view("VPORTS 4", i),

            // ── VPORTS — list or create preset viewport configurations ────
            cmd if cmd == "VPORTS" || cmd.starts_with("VPORTS ") => {
                let sub = cmd.split_whitespace().nth(1).unwrap_or("").to_uppercase();
                let scene = &self.tabs[i].scene;
                if scene.current_layout == "Model" {
                    // Bare VPORTS → ask for the configuration interactively;
                    // the next command-line entry supplies it.
                    if sub.is_empty() {
                        self.awaiting_vports = true;
                        self.command_line
                            .push_info("VPORTS  Configuration [SIngle/2H/2V/4]:");
                        return Some(self.focus_cmd_input());
                    }
                    // Model space: split the tiled viewport layout via pane_grid.
                    use iced::widget::pane_grid::{Axis, Configuration as C};
                    let split = |axis, a, b| C::Split {
                        axis,
                        ratio: 0.5,
                        a: Box::new(a),
                        b: Box::new(b),
                    };
                    let config: Option<(C<usize>, usize)> = match sub.as_str() {
                        "SINGLE" | "SI" | "1" => Some((C::Pane(0), 1)),
                        "2H" | "2" => {
                            Some((split(Axis::Horizontal, C::Pane(0), C::Pane(1)), 2))
                        }
                        "2V" => Some((split(Axis::Vertical, C::Pane(0), C::Pane(1)), 2)),
                        "4" => Some((
                            split(
                                Axis::Vertical,
                                split(Axis::Horizontal, C::Pane(0), C::Pane(2)),
                                split(Axis::Horizontal, C::Pane(1), C::Pane(3)),
                            ),
                            4,
                        )),
                        _ => None,
                    };
                    match config {
                        Some((config, n)) => {
                            self.tabs[i].scene.set_model_panes(config);
                            self.tabs[i].scene.camera_generation += 1;
                            self.command_line
                                .push_output(&format!("VPORTS: {n} viewport(s)."));
                        }
                        None => {
                            self.command_line
                                .push_error("VPORTS: use SINGLE | 2H | 2V | 4.");
                        }
                    }
                } else if sub.is_empty() {
                    // ── List existing viewports ──────────────────────────
                    let layout_block = scene.current_layout_block_handle_pub();
                    let viewports: Vec<_> = scene
                        .document
                        .entities()
                        .filter_map(|e| {
                            if let acadrust::EntityType::Viewport(vp) = e {
                                if vp.id > 1 && vp.common.owner_handle == layout_block {
                                    Some((
                                        vp.id,
                                        vp.center.clone(),
                                        vp.width,
                                        vp.height,
                                        crate::scene::vp_effective_scale(
                                            vp.custom_scale,
                                            vp.view_height,
                                            vp.height,
                                        ),
                                        vp.status.is_on,
                                        vp.status.locked,
                                    ))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                        .collect();
                    if viewports.is_empty() {
                        self.command_line.push_info("No viewports. Use MVIEW to create one, or VPORTS 2H / 2V / 4 / SINGLE.");
                    } else {
                        self.command_line.push_output(&format!(
                            "{} viewport(s) in layout \"{}\":",
                            viewports.len(),
                            scene.current_layout
                        ));
                        for (id, center, w, h, scale, is_on, locked) in &viewports {
                            let state = match (is_on, locked) {
                                (true, true) => "On, Locked",
                                (true, false) => "On",
                                (false, _) => "Off",
                            };
                            self.command_line.push_output(&format!(
                                "  VP #{id}: {w:.1}×{h:.1} @ ({:.1},{:.1})  scale={scale:.4}  [{state}]",
                                center.x, center.y
                            ));
                        }
                    }
                } else {
                    // ── Preset viewport layout ───────────────────────────
                    // Determine paper dimensions from PlotSettings (fallback A4 landscape).
                    let layout_name = scene.current_layout.clone();
                    let (paper_w, paper_h) = {
                        use acadrust::objects::ObjectType;
                        let mut pw = 297.0_f64;
                        let mut ph = 210.0_f64;
                        for (_, obj) in &scene.document.objects {
                            if let ObjectType::PlotSettings(ps) = obj {
                                if ps.page_name == layout_name && ps.paper_width > 0.0 {
                                    pw = ps.paper_width;
                                    ph = ps.paper_height;
                                    break;
                                }
                            }
                        }
                        (pw, ph)
                    };
                    let margin = 5.0_f64; // mm margin around the usable area
                    let uw = paper_w - 2.0 * margin; // usable width
                    let uh = paper_h - 2.0 * margin; // usable height
                                                     // Collect rectangle specs: (cx, cz, w, h) in mm
                    let rects: Vec<(f64, f64, f64, f64)> = match sub.as_str() {
                        "2H" => {
                            // Two viewports side by side (horizontal split)
                            let vw = (uw - 2.0) / 2.0;
                            vec![
                                (margin + vw / 2.0, margin + uh / 2.0, vw, uh),
                                (margin + vw + 2.0 + vw / 2.0, margin + uh / 2.0, vw, uh),
                            ]
                        }
                        "2V" => {
                            // Two viewports stacked (vertical split)
                            let vh = (uh - 2.0) / 2.0;
                            vec![
                                (margin + uw / 2.0, margin + vh + 2.0 + vh / 2.0, uw, vh),
                                (margin + uw / 2.0, margin + vh / 2.0, uw, vh),
                            ]
                        }
                        "4" => {
                            // Four equal viewports (2×2 grid)
                            let vw = (uw - 2.0) / 2.0;
                            let vh = (uh - 2.0) / 2.0;
                            vec![
                                (margin + vw / 2.0, margin + vh + 2.0 + vh / 2.0, vw, vh),
                                (
                                    margin + vw + 2.0 + vw / 2.0,
                                    margin + vh + 2.0 + vh / 2.0,
                                    vw,
                                    vh,
                                ),
                                (margin + vw / 2.0, margin + vh / 2.0, vw, vh),
                                (margin + vw + 2.0 + vw / 2.0, margin + vh / 2.0, vw, vh),
                            ]
                        }
                        "SINGLE" | "1" => {
                            // Single full-page viewport
                            vec![(margin + uw / 2.0, margin + uh / 2.0, uw, uh)]
                        }
                        _ => {
                            self.command_line.push_error(
                                "VPORTS: unknown option. Use VPORTS 2H | 2V | 4 | SINGLE",
                            );
                            vec![]
                        }
                    };
                    if !rects.is_empty() {
                        // Remove existing user viewports in this layout first.
                        let layout_block = self.tabs[i].scene.current_layout_block_handle_pub();
                        let to_erase: Vec<acadrust::Handle> = self.tabs[i]
                            .scene
                            .document
                            .entities()
                            .filter_map(|e| {
                                if let acadrust::EntityType::Viewport(vp) = e {
                                    if vp.id > 1 && vp.common.owner_handle == layout_block {
                                        Some(vp.common.handle)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            })
                            .collect();
                        self.push_undo_snapshot(i, "VPORTS");
                        self.tabs[i].scene.erase_entities(&to_erase);
                        // Create new viewports.
                        for (cx, cz, w, h) in &rects {
                            let mut vp = acadrust::entities::Viewport::new();
                            vp.center = acadrust::types::Vector3::new(*cx, 0.0, *cz);
                            vp.width = *w;
                            vp.height = *h;
                            vp.id = 2; // commit_entity will assign unique IDs
                            match self.tabs[i].scene.document.add_entity_to_layout(
                                acadrust::EntityType::Viewport(vp),
                                &layout_name,
                            ) {
                                Ok(handle) => {
                                    self.tabs[i].scene.auto_fit_viewport(handle);
                                }
                                Err(e) => {
                                    self.command_line.push_error(&format!("VPORTS: {e}"));
                                }
                            }
                        }
                        // Re-assign unique IDs (1 + existing max per viewport).
                        let layout_block2 = self.tabs[i].scene.current_layout_block_handle_pub();
                        let mut id_counter = 2_i16;
                        let handles: Vec<acadrust::Handle> = self.tabs[i]
                            .scene
                            .document
                            .entities()
                            .filter_map(|e| {
                                if let acadrust::EntityType::Viewport(vp) = e {
                                    if vp.id >= 2 && vp.common.owner_handle == layout_block2 {
                                        Some(vp.common.handle)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            })
                            .collect();
                        for h in handles {
                            if let Some(acadrust::EntityType::Viewport(vp)) =
                                self.tabs[i].scene.document.get_entity_mut(h)
                            {
                                vp.id = id_counter;
                                id_counter += 1;
                            }
                        }
                        self.tabs[i].dirty = true;
                        self.command_line.push_output(&format!(
                            "VPORTS: created {} viewport(s) [{}].",
                            rects.len(),
                            sub
                        ));
                    }
                }
            }

            // ── VPLAYER — per-viewport layer freeze/thaw ──────────────────
            "VPLAYER" => {
                let scene = &self.tabs[i].scene;
                if scene.current_layout == "Model" {
                    self.command_line
                        .push_error("VPLAYER: switch to a paper space layout first.");
                } else if scene.active_viewport.is_none() {
                    self.command_line
                        .push_error("VPLAYER: enter a viewport first (double-click or MS).");
                } else {
                    use crate::modules::layout::vplayer::VplayerCommand;
                    let vp_handle = scene.active_viewport.unwrap();
                    // Collect current frozen layer names for display.
                    let frozen_names: Vec<String> = {
                        if let Some(acadrust::EntityType::Viewport(vp)) =
                            scene.document.get_entity(vp_handle)
                        {
                            vp.frozen_layers
                                .iter()
                                .filter_map(|h| {
                                    scene
                                        .document
                                        .layers
                                        .iter()
                                        .find(|l| l.handle == *h)
                                        .map(|l| l.name.clone())
                                })
                                .collect()
                        } else {
                            vec![]
                        }
                    };
                    if frozen_names.is_empty() {
                        self.command_line
                            .push_info("VPLAYER: no frozen layers in active viewport.");
                    } else {
                        self.command_line.push_info(&format!(
                            "VPLAYER: frozen layers: {}",
                            frozen_names.join(", ")
                        ));
                    }
                    let new_cmd = VplayerCommand::new(vp_handle);
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            // ── Draw Order ────────────────────────────────────────────────
            cmd if cmd.starts_with("DRAWORDER") => {
                use acadrust::objects::{ObjectType, SortEntitiesTable};
                let parts: Vec<&str> = cmd.split_whitespace().collect();
                let option = parts.get(1).unwrap_or(&"").to_uppercase();
                let i = self.active_tab;
                let selected: Vec<acadrust::Handle> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .iter()
                    .map(|(h, _)| *h)
                    .collect();
                if selected.is_empty() {
                    self.command_line
                        .push_error("DRAWORDER: select entities first.");
                } else {
                    // Parse relative target handle for ABOVE/UNDER.
                    let relative_target: Option<(bool, acadrust::Handle)> = match option.as_str() {
                        "A" | "ABOVE" => {
                            let h_val = parts.get(2).and_then(|s| u64::from_str_radix(s, 16).ok());
                            h_val.map(|v| (true, acadrust::Handle::new(v)))
                        }
                        "U" | "UNDER" | "BELOW" => {
                            let h_val = parts.get(2).and_then(|s| u64::from_str_radix(s, 16).ok());
                            h_val.map(|v| (false, acadrust::Handle::new(v)))
                        }
                        _ => None,
                    };
                    let to_front_opt = match option.as_str() {
                        "F" | "FRONT" => Some(true),
                        "B" | "BACK" => Some(false),
                        _ => None,
                    };

                    if relative_target.is_some() || to_front_opt.is_some() {
                        self.push_undo_snapshot(i, "DRAWORDER");
                        let block_handle = self.tabs[i].scene.current_layout_block_handle_pub();

                        // For FRONT/BACK, anchor the new sort handle to the
                        // block's current effective draw-order range so the moved
                        // entities land strictly above/below every sibling —
                        // including ones not yet in the table, which sort by
                        // their own handle. (min_eff, max_eff) over siblings.
                        let fb_baseline: Option<(u64, u64)> = if to_front_opt.is_some() {
                            let selected_set: rustc_hash::FxHashSet<u64> =
                                selected.iter().map(|h| h.value()).collect();
                            let doc_ref = &self.tabs[i].scene.document;
                            let overrides: rustc_hash::FxHashMap<u64, u64> = doc_ref
                                .objects
                                .values()
                                .find_map(|obj| {
                                    if let ObjectType::SortEntitiesTable(t) = obj {
                                        if t.block_owner_handle == block_handle {
                                            return Some(
                                                t.entries()
                                                    .map(|e| {
                                                        (
                                                            e.entity_handle.value(),
                                                            e.sort_handle.value(),
                                                        )
                                                    })
                                                    .collect(),
                                            );
                                        }
                                    }
                                    None
                                })
                                .unwrap_or_default();
                            let mut max_eff = 0u64;
                            let mut min_eff = u64::MAX;
                            for e in doc_ref.entities() {
                                let c = e.common();
                                let hv = c.handle.value();
                                if selected_set.contains(&hv) {
                                    continue;
                                }
                                if c.owner_handle != block_handle && !c.owner_handle.is_null() {
                                    continue;
                                }
                                let eff = overrides.get(&hv).copied().unwrap_or(hv);
                                max_eff = max_eff.max(eff);
                                min_eff = min_eff.min(eff);
                            }
                            if min_eff == u64::MAX {
                                min_eff = 1;
                            }
                            Some((min_eff, max_eff))
                        } else {
                            None
                        };

                        let doc = &mut self.tabs[i].scene.document;
                        let table_handle = doc.objects.iter().find_map(|(h, obj)| {
                            if let ObjectType::SortEntitiesTable(t) = obj {
                                if t.block_owner_handle == block_handle {
                                    Some(*h)
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        });
                        let get_or_create =
                            |doc: &mut acadrust::CadDocument, block_handle| -> acadrust::Handle {
                                if let Some(th) = doc.objects.iter().find_map(|(h, obj)| {
                                    if let ObjectType::SortEntitiesTable(t) = obj {
                                        if t.block_owner_handle == block_handle {
                                            Some(*h)
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                }) {
                                    th
                                } else {
                                    let nh = acadrust::Handle::new(doc.next_handle());
                                    let mut table = SortEntitiesTable::for_block(block_handle);
                                    table.handle = nh;
                                    doc.objects.insert(nh, ObjectType::SortEntitiesTable(table));
                                    nh
                                }
                            };
                        let th = table_handle.unwrap_or_else(|| {
                            let nh = acadrust::Handle::new(doc.next_handle());
                            let mut table = SortEntitiesTable::for_block(block_handle);
                            table.handle = nh;
                            doc.objects.insert(nh, ObjectType::SortEntitiesTable(table));
                            nh
                        });
                        let _ = get_or_create; // suppress unused warning
                        if let Some(ObjectType::SortEntitiesTable(table)) = doc.objects.get_mut(&th)
                        {
                            if let Some((above, target)) = relative_target {
                                // move_above/move_below read the target's sort
                                // handle from the table and no-op when it is
                                // absent. A reference object that was never
                                // reordered isn't in the table yet, so seed it
                                // with its own handle as the implicit sort key.
                                if !table.contains(target) {
                                    table.add_entry(target, target);
                                }
                                for h in &selected {
                                    if above {
                                        table.move_above(*h, target);
                                    } else {
                                        table.move_below(*h, target);
                                    }
                                }
                                let rel = if above { "above" } else { "below" };
                                self.command_line.push_info(&format!(
                                    "DRAWORDER: moved {} entities {} {:x}.",
                                    selected.len(),
                                    rel,
                                    target.value()
                                ));
                            } else if let Some(to_front) = to_front_opt {
                                let (min_eff, max_eff) = fb_baseline.unwrap_or((1, 0));
                                for (k, h) in selected.iter().enumerate() {
                                    let sort = if to_front {
                                        max_eff.saturating_add(1 + k as u64)
                                    } else {
                                        min_eff.saturating_sub(1 + k as u64).max(1)
                                    };
                                    table.add_entry(*h, acadrust::Handle::new(sort));
                                }
                                let dir = if to_front { "front" } else { "back" };
                                self.command_line.push_info(&format!(
                                    "DRAWORDER: moved {} entities to {}.",
                                    selected.len(),
                                    dir
                                ));
                            }
                        }
                        // Sort order lives in SortEntitiesTable, which the
                        // render-side `sort_cache` rebuilds per geometry epoch.
                        // Bump it so the new draw order shows immediately
                        // instead of waiting for an unrelated geometry change.
                        self.tabs[i].scene.bump_geometry();
                        self.tabs[i].dirty = true;
                    } else {
                        self.command_line.push_info(
                            "Usage: DRAWORDER F|FRONT | B|BACK | A|ABOVE <handle> | U|UNDER <handle>"
                        );
                    }
                }
            }

            _ => return None,
        }
        Some(self.finish_dispatch(cmd))
    }
}

// ── Draw Order: interactive reference-object pick ──────────────────────────

/// Moves a captured selection above or below a reference object the user
/// picks in the viewport. On pick it relaunches `DRAWORDER A|U <handle>`
/// with the captured handles reinstalled as the selection, so the existing
/// command path performs the actual reorder.
pub(crate) struct DrawOrderRefCommand {
    to_move: Vec<acadrust::Handle>,
    above: bool,
}

impl DrawOrderRefCommand {
    pub(crate) fn new(to_move: Vec<acadrust::Handle>, above: bool) -> Self {
        Self { to_move, above }
    }
}

impl CadCommand for DrawOrderRefCommand {
    fn name(&self) -> &'static str {
        "DRAWORDER"
    }

    fn prompt(&self) -> String {
        if self.above {
            "DRAWORDER  Select reference object (move selection above):".into()
        } else {
            "DRAWORDER  Select reference object (move selection under):".into()
        }
    }

    fn needs_entity_pick(&self) -> bool {
        true
    }

    fn on_entity_pick(
        &mut self,
        handle: acadrust::Handle,
        _pt: glam::DVec3,
    ) -> crate::command::CmdResult {
        if handle.is_null() {
            return crate::command::CmdResult::NeedPoint;
        }
        let opt = if self.above { "A" } else { "U" };
        let cmd = format!("DRAWORDER {} {:x}", opt, handle.value());
        crate::command::CmdResult::Relaunch(cmd, std::mem::take(&mut self.to_move))
    }

    fn on_point(&mut self, _pt: glam::DVec3) -> crate::command::CmdResult {
        crate::command::CmdResult::NeedPoint
    }

    fn on_enter(&mut self) -> crate::command::CmdResult {
        crate::command::CmdResult::Cancel
    }
}
