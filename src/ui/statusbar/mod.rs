//! Bottom status bar — Model/Layout tabs + OSNAP toggle + status info

pub mod statusbar_config;
pub mod statusbar_menu;

use iced::widget::tooltip::Position as TipPos;
use iced::widget::{
    button, container, mouse_area, row, text, text_input, tooltip,
};
use iced::{Background, Border, Color, Element, Length, Theme};

/// Scrollable id of the status-bar layout-tab strip (retained so the existing
/// `Message::ScrollLayoutTabs` handler still resolves; the strip now flex-wraps
/// instead of scrolling).
pub const LAYOUT_TABS_SCROLL_ID: &str = "statusbar-layout-tabs";

/// Widget id of the inline layout-rename text input, so the rename can grab
/// keyboard focus the moment it opens (issue #86).
pub const LAYOUT_RENAME_INPUT_ID: &str = "layout_rename_input";

use crate::app::Message;
use crate::snap::Snapper;
use crate::ui::statusbar::statusbar_config::{StatusBarConfig, StatusPill};
use crate::ui::wrap_bar::{PosReport, WrapBar, WrapFlow};

// PosReport ids for anchoring each status-bar popup directly to its pill.
pub const SB_OSNAP_ID: &str = "SB_OSNAP";
pub const SB_SCALE_ID: &str = "SB_SCALE";
pub const SB_UNITS_ID: &str = "SB_UNITS";
pub const SB_ISOLATE_ID: &str = "SB_ISOLATE";
pub const SB_FILTER_ID: &str = "SB_FILTER";
pub const SB_MENU_ID: &str = "SB_MENU";
pub const SB_LAYOUTLIST_ID: &str = "SB_LAYOUTLIST";
pub const SB_POLAR_ID: &str = "SB_POLAR";

#[derive(Clone, Default)]
pub struct StatusBar {
    #[allow(dead_code)]
    pub coord_display: String,
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            coord_display: "MODEL".into(),
        }
    }

    pub fn view<'a>(
        &'a self,
        snapper: &'a Snapper,
        popup_open: bool,
        ortho_mode: bool,
        polar_mode: bool,
        polar_increment_deg: f32,
        // True while the polar-angle picker popup is open.
        polar_popup_open: bool,
        dyn_input: bool,
        otrack: bool,
        layouts: Vec<String>,
        current_layout: String,
        // If `Some((original, edit_value))`, the named tab shows a text input.
        rename_state: Option<&'a (String, String)>,
        // Scale of the first user viewport in the active paper layout.
        viewport_scale: Option<f64>,
        // Number of user viewports in the current paper layout (0 = model space).
        viewport_count: usize,
        // True when the user is editing inside a paper-space viewport (MSPACE).
        in_mspace: bool,
        // Whether the layout tabs (Model/Paper) are visible (LAYOUTTAB).
        show_layout_tabs: bool,
        // Current annotation scale for model space (1.0 = 1:1, 50.0 = 1:50, etc.).
        annotation_scale: f32,
        // True when the scale picker popup is open.
        scale_popup_open: bool,
        // True when the scale pill is interactive (always model space; paper space only when a viewport is active/selected).
        scale_pill_enabled: bool,
        // LWDISPLAY header flag — controls lineweight visibility in the viewport.
        lineweight_display: bool,
        // Live cursor position in model coordinates, for the coordinate readout.
        cursor_world: glam::DVec3,
        // $COORDS readout mode: 0 = static (updates only on a pick), 1 = live
        // absolute, 2 = polar (distance<angle from the last point while picking).
        coords_mode: i16,
        // The last committed point, for the static (0) and polar (2) readouts.
        last_point: Option<glam::DVec3>,
        // True while a command is prompting for a point (enables the polar readout).
        picking: bool,
        // True while clean-screen mode hides the ribbon and side panels.
        clean_screen: bool,
        // Drawing units (INSUNITS) for the units pill.
        insertion_units: i16,
        // True while the drawing-units picker is open.
        units_popup_open: bool,
        // True when objects are hidden by Isolate / Hide.
        isolation_active: bool,
        // Whether entity transparency is shown (Transparency pill state).
        transparency_display: bool,
        // Whether the Quick Properties floating panel is enabled.
        quick_properties: bool,
        // True when the selection filter is excluding at least one type.
        selection_filter_active: bool,
        // Whether selection cycling is enabled.
        selection_cycling: bool,
        // Which pills the user has chosen to show on the bar.
        config: &'a StatusBarConfig,
    ) -> Element<'a, Message> {
        // Leftmost hamburger: opens a dropdown listing Model + every layout, so
        // a layout can be picked directly even when the tab strip is scrolled.
        let menu_btn = button(crate::ui::icons::tinted(crate::ui::icons::MENU, 16.0, ICON_COLOR))
            .on_press(Message::ToggleLayoutList)
            .style(|_: &Theme, status| button::Style {
                background: Some(Background::Color(match status {
                    button::Status::Hovered => PILL_BG,
                    _ => Color::TRANSPARENT,
                })),
                border: Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .padding([4, 8]);

        let add_btn = button(text("+").size(12).color(ICON_COLOR))
            .on_press(Message::LayoutCreate)
            .style(|_: &Theme, _| button::Style {
                background: Some(Background::Color(Color::TRANSPARENT)),
                ..Default::default()
            })
            .padding([4, 8]);

        // ── Right side ────────────────────────────────────────────────────
        let osnap_active = snapper.is_active();

        let vp_label = if viewport_count > 0 {
            format!("{} VP", viewport_count)
        } else {
            String::new()
        };
        // Scale pill: opens the scale picker popup.
        // Model space: always interactive, shows annotation scale.
        // Paper space: interactive only when a viewport is active/selected.
        let scale_label = if current_layout == "Model" {
            format_scale(Some(1.0 / annotation_scale as f64))
        } else {
            format_scale(viewport_scale)
        };
        let scale_element: Element<'_, Message> = if scale_pill_enabled {
            tip(
                popup_pill(&scale_label, scale_popup_open, Message::ToggleScalePopup),
                "Annotation / Viewport Scale\nClick to change",
            )
        } else {
            status_pill(scale_label).into()
        };
        // Build the right-side pills, honouring the user's per-pill visibility.
        // They live in a flex-wrap flow (WrapFlow) so they spill onto extra rows
        // when the width can't hold them all on one line.
        let vis = |p: StatusPill| config.is_visible(p);
        let mut pills: Vec<Element<'_, Message>> = Vec::new();
        if vis(StatusPill::Coords) {
            let coords_label = format_coords(cursor_world, last_point, coords_mode, picking);
            pills.push(
                tip(
                    popup_pill(&coords_label, false, Message::CycleCoordsMode),
                    "Cursor coordinates ($COORDS)\nClick to cycle: static / live / polar",
                )
                .into(),
            );
        }
        if vis(StatusPill::Ortho) {
            pills.push(
                tip(
                    toggle_pill(crate::ui::icons::ST_ORTHO, ortho_mode, Message::ToggleOrtho),
                    "Orthogonal Mode\nF8",
                )
                .into(),
            );
        }
        if vis(StatusPill::Lwt) {
            pills.push(
                tip(
                    toggle_pill(crate::ui::icons::ST_LWT, lineweight_display, Message::ToggleLineweightDisplay),
                    "Show Lineweight\nLWDISPLAY",
                )
                .into(),
            );
        }
        if vis(StatusPill::Polar) {
            pills.push(
                PosReport::new(
                    SB_POLAR_ID,
                    polar_pill(polar_mode, polar_increment_deg, polar_popup_open),
                )
                .into(),
            );
        }
        if vis(StatusPill::Dyn) {
            pills.push(
                tip(
                    toggle_pill(crate::ui::icons::ST_DYN, dyn_input, Message::ToggleDynInput),
                    "Dynamic Input\nF12",
                )
                .into(),
            );
        }
        if vis(StatusPill::Otrack) {
            pills.push(
                tip(
                    toggle_pill(crate::ui::icons::ST_OTRACK, otrack, Message::ToggleOTrack),
                    "Object Snap Tracking\nF11",
                )
                .into(),
            );
        }
        if vis(StatusPill::Osnap) {
            pills.push(
                PosReport::new(
                    SB_OSNAP_ID,
                    osnap_btn(osnap_active, snapper.snap_enabled, popup_open),
                )
                .into(),
            );
        }
        if vis(StatusPill::Space) {
            pills.push(
                tip(
                    space_mode_btn(&current_layout, in_mspace),
                    "PAPER: double-click viewport to enter MSPACE\nMODEL: click to switch to Model Space",
                )
                .into(),
            );
        }
        if vis(StatusPill::Scale) {
            pills.push(PosReport::new(SB_SCALE_ID, scale_element).into());
        }
        if vis(StatusPill::Units) {
            pills.push(
                PosReport::new(
                    SB_UNITS_ID,
                    tip(
                        popup_pill(
                            crate::ui::popup::units_popup::unit_short(insertion_units),
                            units_popup_open,
                            Message::ToggleUnitsPopup,
                        ),
                        "Drawing Units (INSUNITS)\nClick to change",
                    ),
                )
                .into(),
            );
        }
        if vis(StatusPill::Transparency) {
            pills.push(
                tip(
                    toggle_pill(
                        crate::ui::icons::ST_TRANSPARENCY,
                        transparency_display,
                        Message::ToggleTransparencyDisplay,
                    ),
                    "Show Transparency\nForce opaque when off",
                )
                .into(),
            );
        }
        if vis(StatusPill::Isolate) {
            pills.push(
                PosReport::new(
                    SB_ISOLATE_ID,
                    tip(
                        toggle_pill(crate::ui::icons::ST_ISOLATE, isolation_active, Message::ToggleIsolatePopup),
                        "Isolate Objects\nClick for Isolate / Hide / End",
                    ),
                )
                .into(),
            );
        }
        if vis(StatusPill::QuickProps) {
            pills.push(
                tip(
                    toggle_pill(crate::ui::icons::ST_QUICKPROPS, quick_properties, Message::ToggleQuickProperties),
                    "Quick Properties\nFloating panel on selection",
                )
                .into(),
            );
        }
        if vis(StatusPill::SelFilter) {
            pills.push(
                PosReport::new(
                    SB_FILTER_ID,
                    tip(
                        toggle_pill(
                            crate::ui::icons::ST_FILTER,
                            selection_filter_active,
                            Message::ToggleSelectionFilterPopup,
                        ),
                        "Selection Filtering\nLimit which object types can be picked",
                    ),
                )
                .into(),
            );
        }
        if vis(StatusPill::SelCycle) {
            pills.push(
                tip(
                    toggle_pill(crate::ui::icons::ST_SELCYCLE, selection_cycling, Message::ToggleSelectionCycling),
                    "Selection Cycling\nRepeat-click to step through overlapping objects",
                )
                .into(),
            );
        }
        if vis(StatusPill::Vp) && !vp_label.is_empty() {
            pills.push(
                tip(
                    status_pill(vp_label).into(),
                    "Viewport count in active layout",
                )
                .into(),
            );
        }
        if vis(StatusPill::CleanScreen) {
            pills.push(
                tip(
                    toggle_pill(crate::ui::icons::ST_CLEANSCREEN, clean_screen, Message::ToggleCleanScreen),
                    "Clean Screen\nHide ribbon and panels",
                )
                .into(),
            );
        }
        // Customization handle: opens the pill show/hide menu.
        pills.push(
            PosReport::new(
                SB_MENU_ID,
                tip(
                    customize_btn(),
                    "Customization\nShow or hide status-bar items",
                ),
            )
            .into(),
        );
        let right_status = WrapFlow::new(pills).spacing_x(2.0).row_h(30.0);

        // Left area: hamburger menu + Model/layout tabs in a flex-wrap flow, so
        // they spill onto lower rows when narrow (no scroll arrows). The pills
        // wrap in their own flow; WrapBar stacks the two areas so a wrapped tab
        // never shares a row with a pill.
        let mut left: Vec<Element<'_, Message>> = Vec::new();
        left.push(PosReport::new(SB_LAYOUTLIST_ID, menu_btn).into());
        if show_layout_tabs {
            for name in layouts {
                let is_active = name == current_layout;
                let renaming = rename_state
                    .filter(|(orig, _)| *orig == name)
                    .map(|(_, edit)| edit.as_str());
                left.push(space_tab(name, is_active, renaming).into());
            }
            left.push(add_btn.into());
        }
        let left_area = WrapFlow::new(left).spacing_x(2.0).row_h(30.0);

        let wrap = WrapBar::new(left_area.into(), right_status.into())
            .min_row_h(30.0)
            .justify_end(true);

        container(wrap)
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(BAR_BG)),
                border: Border {
                    color: BORDER_COLOR,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fill)
            // One row matches the drawing (document) tab bar height so the three
            // horizontal strips — tabs, status bar, command line — line up.
            // WrapBar vertically centres every pill (issue #216) and grows to a
            // second row when the width can't hold both blocks.
            .padding([0, 4])
            .into()
    }
}

// ── Coordinate readout ────────────────────────────────────────────────────

fn format_coords(cursor: glam::DVec3, last: Option<glam::DVec3>, mode: i16, picking: bool) -> String {
    let abs = |p: glam::DVec3| format!("{:.4}, {:.4}, {:.4}", p.x, p.y, p.z);
    match mode {
        // Static: show the last picked point; the readout freezes between picks.
        0 => abs(last.unwrap_or(cursor)),
        // Polar: distance < angle relative to the last point while a command is
        // prompting for a point; absolute otherwise.
        2 => match (picking, last) {
            (true, Some(l)) => {
                let d = cursor - l;
                let dist = (d.x * d.x + d.y * d.y).sqrt();
                let mut ang = d.y.atan2(d.x).to_degrees();
                if ang < 0.0 {
                    ang += 360.0;
                }
                format!("{dist:.4} < {ang:.2}\u{b0}")
            }
            _ => abs(cursor),
        },
        // 1 (default) and anything else: live absolute.
        _ => abs(cursor),
    }
}

// ── Customization handle ──────────────────────────────────────────────────

fn customize_btn() -> Element<'static, Message> {
    button(crate::ui::icons::tinted(crate::ui::icons::MENU, 16.0, ICON_COLOR))
        .on_press(Message::ToggleStatusBarMenu)
        .style(|_: &Theme, status| button::Style {
            background: Some(Background::Color(match status {
                button::Status::Hovered => PILL_BG,
                _ => Color::TRANSPARENT,
            })),
            border: Border {
                radius: 3.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .padding([4, 8])
        .into()
}

// ── Tooltip helper ────────────────────────────────────────────────────────

fn tip<'a>(content: Element<'a, Message>, label: &'static str) -> Element<'a, Message> {
    tip_node(content, text(label).size(11).color(Color::WHITE).into())
}

/// Like [`tip`] but the tooltip body is any element — used to embed an SVG
/// glyph (e.g. the dropdown caret) instead of a Unicode character that renders
/// as tofu on the web. (#138)
fn tip_node<'a>(content: Element<'a, Message>, body: Element<'a, Message>) -> Element<'a, Message> {
    tooltip(
        content,
        container(body)
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(Color {
                    r: 0.13,
                    g: 0.13,
                    b: 0.13,
                    a: 0.97,
                })),
                border: Border {
                    color: Color {
                        r: 0.35,
                        g: 0.35,
                        b: 0.35,
                        a: 1.0,
                    },
                    width: 1.0,
                    radius: 3.0.into(),
                },
                text_color: Some(Color::WHITE),
                ..Default::default()
            })
            .padding([4, 8]),
        TipPos::Top,
    )
    .into()
}

// ── Simple toggle pill ────────────────────────────────────────────────────

/// A status-bar toggle, drawn as a tinted icon (issue #216: the old size-10
/// text labels were too small to read). The name lives in the tooltip each
/// call site already wraps it with.
fn toggle_pill(icon: &'static [u8], active: bool, msg: Message) -> Element<'static, Message> {
    let color = if active { OSNAP_ON_TEXT } else { OSNAP_OFF_TEXT };
    button(crate::ui::icons::tinted(icon, 17.0, color))
        .on_press(msg)
        .style(move |_: &Theme, status| button::Style {
            background: Some(Background::Color(match (active, status) {
                (true, button::Status::Hovered) => SNAP_ON_HOVER,
                (true, _) => SNAP_ON_BG,
                (false, button::Status::Hovered) => SNAP_OFF_HOVER,
                (false, _) => SNAP_OFF_BG,
            })),
            border: Border {
                color: if active { SNAP_BORDER_ON } else { BORDER_COLOR },
                width: 1.0,
                radius: 2.0.into(),
            },
            text_color: color,
            shadow: iced::Shadow::default(),
            snap: false,
        })
        .padding([4, 7])
        .into()
}

// ── Split pill (toggle + dropdown caret) ──────────────────────────────────
//
// Shared chrome for every status-bar pill that pairs a toggle with a picker
// popup (OSNAP, POLAR, …): one outer border wraps the caller's `main` region
// and a dropdown caret, so both halves read as a single integrated control and
// the border / background / caret / sizing live in exactly ONE place.

/// Pill text/icon tint for the lit vs. dim state — shared so `main` (built by
/// the caller) and the caret always match.
fn pill_text_color(active: bool) -> Color {
    if active {
        OSNAP_ON_TEXT
    } else {
        OSNAP_OFF_TEXT
    }
}

/// Wrap a caller-built, already-click-wired `main` element together with a
/// dropdown caret that emits `caret_msg`. `active` lights the pill; `open`
/// highlights the border while its popup is showing.
fn split_pill(
    main: Element<'static, Message>,
    caret_msg: Message,
    active: bool,
    open: bool,
) -> Element<'static, Message> {
    let color = pill_text_color(active);
    let bg = if active { SNAP_ON_BG } else { SNAP_OFF_BG };
    let border_color = if open {
        ACCENT
    } else if active {
        SNAP_BORDER_ON
    } else {
        BORDER_COLOR
    };

    let caret = mouse_area(
        container(crate::ui::icons::arrow_down(9.0, color)).padding([0, 1]),
    )
    .on_press(caret_msg);

    container(row![main, caret].spacing(3).align_y(iced::Center))
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                color: border_color,
                width: 1.0,
                radius: 2.0.into(),
            },
            ..Default::default()
        })
        .padding([4, 6])
        .into()
}

// ── Polar tracking pill ───────────────────────────────────────────────────
//
// Main: left-click toggles polar on/off; right-click cycles the increment.
// Caret: opens the angle picker.

fn polar_pill(active: bool, increment_deg: f32, open: bool) -> Element<'static, Message> {
    let angle = crate::ui::popup::polar_popup::angle_label(increment_deg);
    let tooltip_text = format!(
        "Polar Tracking ({angle})\nF10 — left-click on/off\nRight-click cycles · ▾ picks angle",
    );
    let color = pill_text_color(active);

    // Right-click quick-cycles through the same increments the picker lists.
    const CYCLE: &[f32] = &[90.0, 45.0, 30.0, 22.5, 18.0, 15.0, 10.0, 5.0, 1.0];
    let next_angle = match CYCLE.iter().position(|&a| (a - increment_deg).abs() < 1e-3) {
        Some(i) => CYCLE[(i + 1) % CYCLE.len()],
        None => 45.0,
    };

    let main = mouse_area(
        row![
            crate::ui::icons::tinted(crate::ui::icons::ST_POLAR, 17.0, color),
            text(angle).size(11).color(color),
        ]
        .spacing(2)
        .align_y(iced::Center),
    )
    .on_press(Message::TogglePolar)
    .on_right_press(Message::SetPolarAngle(next_angle));

    tooltip(
        split_pill(main.into(), Message::TogglePolarPopup, active, open),
        container(text(tooltip_text).size(11).color(Color::WHITE))
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(Color {
                    r: 0.13,
                    g: 0.13,
                    b: 0.13,
                    a: 0.95,
                })),
                border: Border {
                    color: Color {
                        r: 0.35,
                        g: 0.35,
                        b: 0.35,
                        a: 1.0,
                    },
                    width: 1.0,
                    radius: 3.0.into(),
                },
                ..Default::default()
            })
            .padding([4, 8]),
        TipPos::Top,
    )
    .into()
}

// ── OSNAP pill ─────────────────────────────────────────────────────────────
//
// Main: toggles the global snap on/off. Caret: opens the snap-type dropdown.

fn osnap_btn(active: bool, snap_enabled: bool, open: bool) -> Element<'static, Message> {
    let on = active || snap_enabled;
    let main = mouse_area(crate::ui::icons::tinted(
        crate::ui::icons::ST_OSNAP,
        17.0,
        pill_text_color(on),
    ))
    .on_press(Message::ToggleSnapEnabled);

    tip(
        split_pill(main.into(), Message::ToggleSnapPopup, on, open),
        "Object Snap: toggle on/off · ▾ opens the snap list\nF3",
    )
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// A layout tab button.
///
/// When `rename_edit` is `Some(value)` the tab shows an inline text input
/// instead of the normal button.  The tab is not renameable when it is the
/// "Model" tab (callers simply never pass `Some` for that name).
fn space_tab<'a>(
    label: String,
    is_active: bool,
    rename_edit: Option<&'a str>,
) -> Element<'a, Message> {
    let bg = move |is_active: bool, hovered: bool| {
        if is_active {
            TAB_ACTIVE
        } else if hovered {
            TAB_HOVER
        } else {
            Color::TRANSPARENT
        }
    };

    let border = Border {
        color: if is_active {
            ACCENT
        } else {
            Color::TRANSPARENT
        },
        width: if is_active { 1.0 } else { 0.0 },
        radius: 2.0.into(),
    };

    let text_color = if is_active {
        Color::WHITE
    } else {
        Color {
            r: 0.65,
            g: 0.65,
            b: 0.65,
            a: 1.0,
        }
    };

    if let Some(edit_val) = rename_edit {
        // Inline rename text input with a cancel (✕) button.
        let input = text_input("", edit_val)
            .id(iced::widget::Id::new(LAYOUT_RENAME_INPUT_ID))
            .on_input(Message::LayoutRenameEdit)
            .on_submit(Message::LayoutRenameCommit)
            .size(12)
            .style(|_: &Theme, _| text_input::Style {
                background: Background::Color(TAB_ACTIVE),
                border: Border {
                    color: ACCENT,
                    width: 1.0,
                    radius: 2.0.into(),
                },
                icon: Color::WHITE,
                placeholder: Color {
                    r: 0.5,
                    g: 0.5,
                    b: 0.5,
                    a: 1.0,
                },
                value: Color::WHITE,
                selection: Color {
                    r: 0.20,
                    g: 0.55,
                    b: 0.90,
                    a: 0.4,
                },
            })
            .padding([3, 6])
            .width(Length::Fixed(90.0));

        let cancel_btn = button(crate::ui::icons::tinted(
            crate::ui::icons::CLOSE,
            10.0,
            Color {
                r: 0.65,
                g: 0.65,
                b: 0.65,
                a: 1.0,
            },
        ))
        .on_press(Message::LayoutRenameCancel)
        .style(|_: &Theme, _| button::Style {
            background: Some(Background::Color(Color::TRANSPARENT)),
            border: Border::default(),
            shadow: iced::Shadow::default(),
            snap: false,
            ..Default::default()
        })
        .padding([4, 4]);

        row![input, cancel_btn]
            .spacing(0)
            .align_y(iced::Center)
            .into()
    } else {
        // Normal clickable tab — left click switches, right click opens context menu.
        let display = container(text(label.clone()).size(12).color(text_color))
            .style(move |_: &Theme| container::Style {
                background: Some(Background::Color(bg(is_active, false))),
                border,
                ..Default::default()
            })
            .padding([4, 10]);

        let switch_msg = Message::LayoutSwitch(label.clone());
        let ctx_msg = Message::LayoutContextMenu(label.clone());

        // Use mouse_area so we can capture right-click for the context menu.
        // PosReport records the tab's screen bounds so the context menu can
        // anchor next to the clicked tab instead of the screen's left edge
        // (#428).
        crate::ui::wrap_bar::PosReport::owned(
            format!("SB_LAYOUT_TAB:{label}"),
            mouse_area(display)
                .on_press(switch_msg)
                .on_right_press(ctx_msg),
        )
        .into()
    }
}

/// Space-mode toggle button in the status bar.
///
/// - Model tab            → "MODEL"  (non-clickable, informational)
/// - Layout, PSPACE       → "PAPER"  (click → MspaceCommand: enter MSPACE)
/// - Layout, MSPACE       → "MODEL"  (click → ExitViewport: return to PSPACE)
fn space_mode_btn(current_layout: &str, in_mspace: bool) -> Element<'static, Message> {
    let is_model_tab = current_layout == "Model";

    // Labels and styling follow AutoCAD convention:
    //   PAPER = currently in paper-space editing
    //   MODEL = currently in model-space editing (either the Model tab or MSPACE)
    let (label, active, on_press) = if is_model_tab {
        ("MODEL", false, None::<Message>)
    } else if in_mspace {
        ("MODEL", true, Some(Message::ExitViewport))
    } else {
        ("PAPER", false, Some(Message::MspaceCommand))
    };

    let text_color = if active {
        SNAP_BORDER_ON
    } else {
        OSNAP_OFF_TEXT
    };
    let bg_normal = if active { SNAP_ON_BG } else { SNAP_OFF_BG };
    let bg_hover = if active {
        SNAP_ON_HOVER
    } else {
        SNAP_OFF_HOVER
    };
    let border_color = if active { SNAP_BORDER_ON } else { BORDER_COLOR };

    let clickable = on_press.is_some();
    let mut btn = button(text(label).size(12).color(text_color))
        .style(move |_: &Theme, status| button::Style {
            background: Some(Background::Color(match status {
                button::Status::Hovered if clickable => bg_hover,
                _ => bg_normal,
            })),
            border: Border {
                color: border_color,
                width: 1.0,
                radius: 2.0.into(),
            },
            text_color,
            shadow: iced::Shadow::default(),
            snap: false,
        })
        .padding([4, 7]);

    if let Some(msg) = on_press {
        btn = btn.on_press(msg);
    }

    btn.into()
}

fn status_pill(label: impl Into<String>) -> Element<'static, Message> {
    container(text(label.into()).size(12).color(Color {
        r: 0.65,
        g: 0.65,
        b: 0.65,
        a: 1.0,
    }))
    .style(|_: &Theme| container::Style {
        background: Some(Background::Color(PILL_BG)),
        border: Border {
            color: BORDER_COLOR,
            width: 1.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    })
    .padding([4, 8])
    .into()
}

// ── Colours ───────────────────────────────────────────────────────────────

const BAR_BG: Color = Color {
    r: 0.14,
    g: 0.14,
    b: 0.14,
    a: 1.0,
};
const TAB_ACTIVE: Color = Color {
    r: 0.25,
    g: 0.25,
    b: 0.25,
    a: 1.0,
};
const TAB_HOVER: Color = Color {
    r: 0.20,
    g: 0.20,
    b: 0.20,
    a: 1.0,
};
const PILL_BG: Color = Color {
    r: 0.19,
    g: 0.19,
    b: 0.19,
    a: 1.0,
};
const BORDER_COLOR: Color = Color {
    r: 0.28,
    g: 0.28,
    b: 0.28,
    a: 1.0,
};
const ICON_COLOR: Color = Color {
    r: 0.70,
    g: 0.70,
    b: 0.70,
    a: 1.0,
};
const ACCENT: Color = Color {
    r: 0.20,
    g: 0.55,
    b: 0.90,
    a: 1.0,
};

const OSNAP_ON_TEXT: Color = Color {
    r: 0.35,
    g: 0.75,
    b: 1.00,
    a: 1.0,
};
const OSNAP_OFF_TEXT: Color = Color {
    r: 0.42,
    g: 0.42,
    b: 0.42,
    a: 1.0,
};
const SNAP_ON_BG: Color = Color {
    r: 0.10,
    g: 0.20,
    b: 0.32,
    a: 1.0,
};
const SNAP_ON_HOVER: Color = Color {
    r: 0.14,
    g: 0.27,
    b: 0.42,
    a: 1.0,
};
const SNAP_BORDER_ON: Color = Color {
    r: 0.20,
    g: 0.50,
    b: 0.85,
    a: 1.0,
};
const SNAP_OFF_BG: Color = Color {
    r: 0.17,
    g: 0.17,
    b: 0.17,
    a: 1.0,
};
const SNAP_OFF_HOVER: Color = Color {
    r: 0.22,
    g: 0.22,
    b: 0.22,
    a: 1.0,
};

// ── Scale popup button ────────────────────────────────────────────────────

/// A labelled status-bar pill that opens a picker popup on click (units,
/// scale, …). Lit + accent-bordered while its popup is `open`. Shared so the
/// popup-opening pills don't each re-declare the same button chrome.
fn popup_pill(label: &str, open: bool, msg: Message) -> Element<'static, Message> {
    let label = label.to_string();
    button(
        text(label)
            .size(12)
            .color(if open { SNAP_BORDER_ON } else { OSNAP_OFF_TEXT }),
    )
    .on_press(msg)
    .style(move |_: &Theme, status| button::Style {
        background: Some(Background::Color(match (open, status) {
            (true, button::Status::Hovered) => SNAP_ON_HOVER,
            (true, _) => SNAP_ON_BG,
            (false, button::Status::Hovered) => SNAP_OFF_HOVER,
            (false, _) => SNAP_OFF_BG,
        })),
        border: Border {
            color: if open { SNAP_BORDER_ON } else { BORDER_COLOR },
            width: 1.0,
            radius: 2.0.into(),
        },
        text_color: if open { SNAP_BORDER_ON } else { OSNAP_OFF_TEXT },
        shadow: iced::Shadow::default(),
        snap: false,
    })
    .padding([4, 7])
    .into()
}

// ── Scale display ─────────────────────────────────────────────────────────

/// Formats a viewport scale factor as a human-readable ratio string.
///
/// - `None`  → "1:1"  (model space or no viewport yet)
/// - `1.0`   → "1:1"
/// - `0.02`  → "1:50"
/// - `2.0`   → "2:1"
fn format_scale(scale: Option<f64>) -> String {
    let s = match scale {
        None => return "1:1".to_string(),
        Some(v) if v <= 0.0 => return "1:1".to_string(),
        Some(v) => v,
    };

    // Try to express as a clean integer ratio.
    if s >= 1.0 {
        let n = s.round() as u32;
        if (s - n as f64).abs() < 0.01 * s {
            return if n == 1 {
                "1:1".to_string()
            } else {
                format!("{}:1", n)
            };
        }
    } else {
        let inv = (1.0 / s).round() as u32;
        if (s - 1.0 / inv as f64).abs() < 0.01 * s {
            return format!("1:{}", inv);
        }
    }

    // Fall back to a decimal string.
    format!("{:.4}", s)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}
