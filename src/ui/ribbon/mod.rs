// Ribbon — tab bar + 3-row tool area.
//
// Button sizes:
//   LargeTool / LargeDropdown  — full ribbon height (3 rows), icon + label [+ ▾]
//   Tool / Dropdown            — 1-row height, icon only [+ ▾ on right]
//
// Dropdown items within a group are collected into columns of 3 rows.

use rustc_hash::FxHashMap as HashMap;

use acadrust::types::{Color as AcadColor, LineWeight};
use iced::widget::{button, column, container, mouse_area, row, scrollable, svg, text};
use iced::{Background, Border, Color, Element, Fill, Length, Padding, Theme};

use crate::app::Message;
use crate::modules::{CadModule, IconKind, RibbonItem};
use crate::plugin::all_ribbon_modules;
use crate::ui::properties::{lw_options, LinetypeItem};

mod widgets;
use widgets::{StyleContext, *};

// ── Ribbon state ───────────────────────────────────────────────────────────

pub struct Ribbon {
    modules: Vec<Box<dyn CadModule>>,
    active: usize,
    active_tool: Option<String>,
    pub wireframe: bool,
    pub ortho_mode: bool,
    pub open_dropdown: Option<String>,
    last_cmd: HashMap<&'static str, &'static str>,
    pub layer_names: Vec<String>,
    pub active_layer: String,
    pub layer_infos: Vec<LayerInfo>,
    /// Active object color — ACI / ByLayer / ByBlock.
    pub active_color: AcadColor,
    /// Active linetype override ("ByLayer", "Continuous", …).
    pub active_linetype: String,
    /// Active lineweight.
    pub active_lineweight: LineWeight,
    /// Linetypes loaded from the current document (with ASCII art).
    pub available_linetypes: Vec<LinetypeItem>,
    /// Whether the full ACI palette is expanded inside the color picker overlay.
    pub prop_color_palette_open: bool,
    // ── Style selector state ──────────────────────────────────────────────
    pub text_style_names: Vec<String>,
    pub active_text_style: String,
    pub dim_style_names: Vec<String>,
    pub active_dim_style: String,
    pub mleader_style_names: Vec<String>,
    pub active_mleader_style: String,
    pub table_style_names: Vec<String>,
    pub active_table_style: String,
}

/// Per-layer display data shown in the ribbon layer dropdown.
#[derive(Clone, Debug)]
pub struct LayerInfo {
    pub name: String,
    pub color: Color,
    pub visible: bool,
    pub frozen: bool,
    pub locked: bool,
}

impl Ribbon {
    pub fn new() -> Self {
        Self {
            modules: all_ribbon_modules(),
            active: 0,
            active_tool: None,
            wireframe: false,
            ortho_mode: true,
            open_dropdown: None,
            last_cmd: HashMap::default(),
            // Empty until a document is open — populated by sync_ribbon_layers.
            layer_names: vec![],
            active_layer: String::new(),
            layer_infos: vec![],
            active_color: AcadColor::ByLayer,
            active_linetype: "ByLayer".to_string(),
            active_lineweight: LineWeight::ByLayer,
            available_linetypes: vec![LinetypeItem {
                name: "Continuous".to_string(),
                art: String::new(),
            }],
            prop_color_palette_open: false,
            // Empty until a document is open — the Annotate style dropdowns
            // are populated from the active document by sync_ribbon_styles.
            text_style_names: vec![],
            active_text_style: String::new(),
            dim_style_names: vec![],
            active_dim_style: String::new(),
            mleader_style_names: vec![],
            active_mleader_style: String::new(),
            table_style_names: vec![],
            active_table_style: String::new(),
        }
    }

    pub fn set_styles(
        &mut self,
        text: Vec<String>,
        active_text: &str,
        dim: Vec<String>,
        active_dim: &str,
        mleader: Vec<String>,
        active_mleader: &str,
        table: Vec<String>,
        active_table: &str,
    ) {
        self.text_style_names = text;
        self.active_text_style = active_text.to_string();
        self.dim_style_names = dim;
        self.active_dim_style = active_dim.to_string();
        self.mleader_style_names = mleader;
        self.active_mleader_style = active_mleader.to_string();
        self.table_style_names = table;
        self.active_table_style = active_table.to_string();
    }

    pub fn set_layers(&mut self, infos: Vec<LayerInfo>, active: &str) {
        self.active_layer = active.to_string();
        self.layer_names = infos.iter().map(|l| l.name.clone()).collect();
        self.layer_infos = infos;
    }

    pub fn set_available_linetypes(&mut self, items: Vec<LinetypeItem>) {
        self.available_linetypes = items;
    }

    pub fn select(&mut self, index: usize) {
        if index < self.modules.len() {
            self.active = index;
        }
    }

    /// Replace the tab list (e.g. after a plugin is enabled/disabled in the
    /// Plugin Manager). Clamps the active tab so it stays in range.
    pub fn set_modules(&mut self, modules: Vec<Box<dyn CadModule>>) {
        self.modules = modules;
        if self.active >= self.modules.len() {
            self.active = self.modules.len().saturating_sub(1);
        }
        self.active_tool = None;
        self.open_dropdown = None;
    }
    pub fn activate_tool(&mut self, id: &str) {
        self.active_tool = Some(id.to_string());
    }
    pub fn deactivate_tool(&mut self) {
        self.active_tool = None;
    }
    /// Clear `active_tool` only when it currently equals `id`. Used by the
    /// window-close path to deactivate the tool that owned a popup window
    /// without disturbing a different tool the user picked in the
    /// meantime. See #40.
    pub fn deactivate_tool_if(&mut self, id: &str) {
        if self.active_tool.as_deref() == Some(id) {
            self.active_tool = None;
        }
    }
    pub fn set_wireframe(&mut self, w: bool) {
        self.wireframe = w;
    }
    pub fn set_ortho(&mut self, ortho: bool) {
        self.ortho_mode = ortho;
    }

    pub fn toggle_dropdown(&mut self, id: &str) {
        if self.open_dropdown.as_deref() == Some(id) {
            self.open_dropdown = None;
        } else {
            self.open_dropdown = Some(id.to_string());
        }
    }
    pub fn close_dropdown(&mut self) {
        self.open_dropdown = None;
    }

    /// Returns the index of the Layout module in the modules list.
    #[allow(dead_code)] // layout-tab helpers; not yet wired
    pub fn layout_module_index(&self) -> Option<usize> {
        self.modules.iter().position(|m| m.id() == "layout")
    }

    /// Returns true if the currently active tab is the Layout module.
    #[allow(dead_code)]
    pub fn active_is_layout(&self) -> bool {
        self.modules
            .get(self.active)
            .map(|m| m.id() == "layout")
            .unwrap_or(false)
    }

    pub fn select_dropdown_item(&mut self, dropdown_id: &'static str, cmd: &'static str) {
        self.last_cmd.insert(dropdown_id, cmd);
        self.open_dropdown = None;
    }

    // ── View ──────────────────────────────────────────────────────────────

    pub fn view(
        &self,
        is_paper: bool,
        undo_count: usize,
        redo_count: usize,
    ) -> Element<'_, Message> {
        // ── Logo ──────────────────────────────────────────────────────────
        let logo_svg = {
            let handle = svg::Handle::from_memory(include_bytes!("../../../assets/logo.svg"));
            svg(handle).width(30).height(28)
        };
        let logo = button(logo_svg)
            .on_press(Message::ToggleAppMenu)
            .style(|_: &Theme, status| button::Style {
                background: Some(Background::Color(match status {
                    button::Status::Hovered | button::Status::Pressed => Color {
                        r: 0.80,
                        g: 0.25,
                        b: 0.15,
                        a: 1.0,
                    },
                    _ => LOGO_RED,
                })),
                border: Border {
                    radius: 2.0.into(),
                    ..Default::default()
                },
                shadow: iced::Shadow::default(),
                snap: false,
                ..Default::default()
            })
            .padding([0, 4]);

        // ── Tab buttons ───────────────────────────────────────────────────
        let history_controls = row![
            render_history_control("Undo", UNDO_HISTORY_ID, undo_count, &self.open_dropdown),
            render_history_control("Redo", REDO_HISTORY_ID, redo_count, &self.open_dropdown),
        ]
        .spacing(TOP_HIST_GAP)
        .align_y(iced::Center);

        let tab_buttons = self.modules.iter().enumerate().fold(
            row![logo, history_controls]
                .align_y(iced::Center)
                .spacing(6),
            |row_acc, (i, module)| {
                // The Layout module no longer has a ribbon tab — its paper-space
                // tools live in the right-edge side toolbar (see ui::side_toolbar).
                if module.id() == "layout" {
                    return row_acc;
                }

                let is_active = i == self.active;
                let is_contextual = module.id() == "layout";
                let accent = if is_contextual {
                    ACCENT_GOLD
                } else {
                    ACCENT_BLUE
                };
                let text_inactive = if is_contextual {
                    Color {
                        r: 0.90,
                        g: 0.72,
                        b: 0.30,
                        a: 1.0,
                    }
                } else {
                    Color {
                        r: 0.75,
                        g: 0.75,
                        b: 0.75,
                        a: 1.0,
                    }
                };
                let hover_bg = if is_contextual {
                    Color {
                        r: 0.28,
                        g: 0.24,
                        b: 0.12,
                        a: 1.0,
                    }
                } else {
                    Color {
                        r: 0.25,
                        g: 0.25,
                        b: 0.25,
                        a: 1.0,
                    }
                };
                let btn = container(
                    button(text(module.title()).size(12))
                        .on_press(Message::RibbonSelectTab(i))
                        .style(move |_: &Theme, status| button::Style {
                            background: Some(Background::Color(match (is_active, status) {
                                (true, _) => RIBBON_BG,
                                (false, button::Status::Hovered) => hover_bg,
                                _ => Color::TRANSPARENT,
                            })),
                            text_color: if is_active {
                                Color::WHITE
                            } else {
                                text_inactive
                            },
                            border: Border {
                                color: if is_active {
                                    accent
                                } else {
                                    Color::TRANSPARENT
                                },
                                width: if is_active { 2.0 } else { 0.0 },
                                radius: 0.0.into(),
                            },
                            shadow: iced::Shadow::default(),
                            snap: false,
                        })
                        .padding([5, 14]),
                )
                .style(move |_: &Theme| container::Style {
                    border: Border {
                        color: if is_active {
                            accent
                        } else {
                            Color::TRANSPARENT
                        },
                        width: if is_active { 2.0 } else { 0.0 },
                        radius: 0.0.into(),
                    },
                    ..Default::default()
                });
                row_acc.push(btn)
            },
        );

        let tab_bar = container(tab_buttons)
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(TOPBAR_BG)),
                ..Default::default()
            })
            .width(Length::Fill)
            .height(28);

        // ── Tool area ─────────────────────────────────────────────────────
        let effective_active = if !is_paper
            && self
                .modules
                .get(self.active)
                .map(|m| m.id() == "layout")
                .unwrap_or(false)
        {
            0
        } else {
            self.active
        };
        let tool_area: Element<'_, Message> =
            if let Some(module) = self.modules.get(effective_active) {
                let groups = module.ribbon_groups();
                let wireframe = self.wireframe;
                let ortho_mode = self.ortho_mode;
                let active_tool = self.active_tool.clone();
                let open_dd = self.open_dropdown.clone();
                let last_cmd = &self.last_cmd;
                let layer_infos = &self.layer_infos;
                let active_layer = &self.active_layer;
                let active_color = self.active_color;
                let active_linetype = &self.active_linetype;
                let active_lineweight = self.active_lineweight;
                let style_ctx = StyleContext {
                    text_style_names: self.text_style_names.clone(),
                    active_text_style: self.active_text_style.clone(),
                    dim_style_names: self.dim_style_names.clone(),
                    active_dim_style: self.active_dim_style.clone(),
                    mleader_style_names: self.mleader_style_names.clone(),
                    active_mleader_style: self.active_mleader_style.clone(),
                    table_style_names: self.table_style_names.clone(),
                    active_table_style: self.active_table_style.clone(),
                };

                let mut widgets: Vec<Element<Message>> = Vec::new();
                let mut first_group = true;

                for group in groups {
                    if !first_group {
                        widgets.push(
                            container(text(""))
                                .width(1)
                                .height(Fill)
                                .style(|_: &Theme| container::Style {
                                    background: Some(Background::Color(BORDER_DARK)),
                                    ..Default::default()
                                })
                                .into(),
                        );
                    }
                    first_group = false;

                    let mut items_row: Vec<Element<Message>> = Vec::new();
                    let mut small_buf: Vec<Element<Message>> = Vec::new();

                    for item in group.tools {
                        let is_large = matches!(
                            &item,
                            RibbonItem::LargeTool(_)
                                | RibbonItem::LargeDropdown { .. }
                                | RibbonItem::LayerComboGroup { .. }
                                | RibbonItem::PropertiesGroup { .. }
                                | RibbonItem::StyleComboGroup { .. }
                        );

                        if is_large {
                            flush_small_col(&mut small_buf, &mut items_row);
                            items_row.push(render_large(
                                item,
                                &active_tool,
                                &open_dd,
                                last_cmd,
                                wireframe,
                                ortho_mode,
                                layer_infos,
                                active_layer,
                                active_color,
                                active_linetype,
                                active_lineweight,
                                &style_ctx,
                            ));
                        } else {
                            small_buf.push(render_small(
                                item,
                                &active_tool,
                                &open_dd,
                                last_cmd,
                                wireframe,
                                ortho_mode,
                            ));
                            if small_buf.len() == 3 {
                                flush_small_col(&mut small_buf, &mut items_row);
                            }
                        }
                    }
                    flush_small_col(&mut small_buf, &mut items_row);

                    let tools_el = items_row
                        .into_iter()
                        .fold(row![].spacing(2).height(Fill).align_y(iced::Top), |r, e| {
                            r.push(e)
                        });

                    widgets.push(
                        column![
                            tools_el,
                            container(text(group.title).size(9).color(GROUP_LABEL)).padding([1, 4]),
                        ]
                        .align_x(iced::Center)
                        .spacing(0)
                        .padding([3u16, 4])
                        .height(Length::Fill)
                        .into(),
                    );
                }

                widgets
                    .into_iter()
                    .fold(row![].spacing(0).height(Length::Fill), |r, w| r.push(w))
                    .into()
            } else {
                text("").into()
            };

        let tool_bar = container(tool_area)
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(RIBBON_BG)),
                border: Border {
                    color: BORDER_DARK,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fill)
            .height(TOOL_BAR_H);

        column![tab_bar, tool_bar].into()
    }

    // ── Dropdown overlay ──────────────────────────────────────────────────

    pub fn dropdown_overlay(
        &self,
        undo_labels: &[String],
        redo_labels: &[String],
    ) -> Option<Element<'_, Message>> {
        let open_id = self.open_dropdown.as_deref()?;

        if open_id == UNDO_HISTORY_ID || open_id == REDO_HISTORY_ID {
            let is_undo = open_id == UNDO_HISTORY_ID;
            let labels = if is_undo { undo_labels } else { redo_labels };
            if labels.is_empty() {
                return None;
            }

            let rows: Vec<Element<Message>> = labels
                .iter()
                .enumerate()
                .map(|(idx, label)| {
                    let step = idx + 1;
                    button(text(label.clone()).size(11).color(LABEL_ON))
                        .on_press(if is_undo {
                            Message::UndoMany(step)
                        } else {
                            Message::RedoMany(step)
                        })
                        .style(|_: &Theme, status| button::Style {
                            background: Some(Background::Color(match status {
                                button::Status::Hovered | button::Status::Pressed => ROW_HOVER,
                                _ => Color::TRANSPARENT,
                            })),
                            ..Default::default()
                        })
                        .width(Fill)
                        .padding([5, 10])
                        .into()
                })
                .collect();

            let panel = container(column(rows))
                .style(|_: &Theme| container::Style {
                    background: Some(Background::Color(PANEL_BG)),
                    border: Border {
                        color: PANEL_BORDER,
                        width: 1.0,
                        radius: 3.0.into(),
                    },
                    ..Default::default()
                })
                .width(Length::Fixed(170.0));

            let positioned = container(panel)
                .align_left(Fill)
                .align_top(Fill)
                .padding(Padding {
                    top: 28.0,
                    left: compute_history_dropdown_left(open_id),
                    ..Default::default()
                })
                .width(Fill)
                .height(Fill);

            return Some(
                mouse_area(positioned)
                    .on_press(Message::CloseRibbonDropdown)
                    .into(),
            );
        }

        if open_id == LAYER_COMBO_ID {
            return self.layer_combo_overlay();
        }

        if open_id == PROP_COLOR_ID {
            return self.prop_color_overlay();
        }
        if open_id == PROP_LINETYPE_ID {
            return self.prop_linetype_overlay();
        }
        if open_id == PROP_LW_ID {
            return self.prop_lw_overlay();
        }

        // Style combo dropdowns (annotate tab) float as overlays so the list
        // isn't clipped by the fixed ribbon-row height, the way the Draw-tab
        // dropdowns already are. (#153)
        if let Some(ov) = self.style_combo_overlay(open_id) {
            return Some(ov);
        }

        let module = self.modules.get(self.active)?;
        let groups = module.ribbon_groups();
        let mut items_list: Option<Vec<(&'static str, &'static str, IconKind)>> = None;
        let mut dd_default = "";
        let mut dd_id: &'static str = "";

        'outer: for group in &groups {
            for item in &group.tools {
                let (id, items, default) = match item {
                    RibbonItem::Dropdown {
                        id, items, default, ..
                    } => (id, items, default),
                    RibbonItem::LargeDropdown {
                        id, items, default, ..
                    } => (id, items, default),
                    _ => continue,
                };
                if *id == open_id {
                    items_list = Some(items.clone());
                    dd_default = default;
                    dd_id = id;
                    break 'outer;
                }
            }
        }
        let items = items_list?;
        let last_cmd = self.last_cmd.get(dd_id).copied().unwrap_or(dd_default);

        let rows: Vec<Element<Message>> = items
            .iter()
            .map(|(cmd, label, item_icon)| {
                let is_current = *cmd == last_cmd;
                let checkmark: Element<'_, Message> = container(if is_current {
                    crate::ui::icons::tinted(crate::ui::icons::CHECK, 11.0, CHECK_COLOR)
                } else {
                    iced::widget::Space::new().width(0).into()
                })
                .width(Length::Fixed(14.0))
                .into();
                let icon_el: Element<Message> = match *item_icon {
                    IconKind::Glyph(s) => text(s)
                        .size(13)
                        .color(ICON_COLOR)
                        .width(Length::Fixed(20.0))
                        .into(),
                    IconKind::Svg(bytes) => {
                        let handle = svg::Handle::from_memory(bytes);
                        svg(handle).width(20).height(20).into()
                    }
                };
                let label_el =
                    text(*label)
                        .size(11)
                        .color(if is_current { LABEL_ON } else { LABEL_OFF });

                button(
                    row![checkmark, icon_el, label_el]
                        .spacing(4)
                        .align_y(iced::Center),
                )
                .on_press(Message::DropdownSelectItem {
                    dropdown_id: dd_id,
                    cmd: *cmd,
                })
                .style(|_: &Theme, status| button::Style {
                    background: Some(Background::Color(match status {
                        button::Status::Hovered | button::Status::Pressed => ROW_HOVER,
                        _ => Color::TRANSPARENT,
                    })),
                    ..Default::default()
                })
                .width(Fill)
                .padding([4, 10])
                .into()
            })
            .collect();

        let panel = container(column(rows))
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(PANEL_BG)),
                border: Border {
                    color: PANEL_BORDER,
                    width: 1.0,
                    radius: 3.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fixed(190.0));

        let top_offset = 28.0 + TOOL_BAR_H;
        let left_offset = compute_dropdown_left(&groups, open_id);
        let positioned = container(panel)
            .align_left(Fill)
            .align_top(Fill)
            .padding(Padding {
                top: top_offset,
                left: left_offset,
                ..Default::default()
            })
            .width(Fill)
            .height(Fill);

        Some(
            mouse_area(positioned)
                .on_press(Message::CloseRibbonDropdown)
                .into(),
        )
    }

    fn layer_combo_overlay(&self) -> Option<Element<'_, Message>> {
        // A toggle icon (visible / freeze / lock) is its own button so a click
        // on it flips that state instead of bubbling up to the row's
        // make-active handler (#133).
        let icon_btn = |bytes: &'static [u8], msg: Message| -> Element<'_, Message> {
            button(crate::ui::icons::raw(bytes, 14.0))
                .on_press(msg)
                .style(|_: &Theme, status| button::Style {
                    background: Some(Background::Color(match status {
                        button::Status::Hovered | button::Status::Pressed => ROW_HOVER,
                        _ => Color::TRANSPARENT,
                    })),
                    ..Default::default()
                })
                .padding([2, 4])
                .into()
        };
        let rows: Vec<Element<Message>> = self
            .layer_infos
            .iter()
            .enumerate()
            .map(|(index, info)| {
                let is_active = info.name == self.active_layer;
                let lc = info.color;
                let lv = info.visible;
                let lf = info.frozen;
                let ll = info.locked;
                let name = info.name.clone();

                let swatch = container(text(""))
                    .style(move |_: &Theme| container::Style {
                        background: Some(Background::Color(lc)),
                        border: Border {
                            color: SWATCH_BORDER,
                            width: 1.0,
                            radius: 1.0.into(),
                        },
                        ..Default::default()
                    })
                    .width(12)
                    .height(12);

                let vis = icon_btn(
                    crate::ui::icons::layer_visible(lv),
                    Message::LayerToggleVisible(index),
                );
                let freeze = icon_btn(
                    crate::ui::icons::layer_freeze(lf),
                    Message::LayerToggleFreeze(index),
                );
                let lock = icon_btn(
                    crate::ui::icons::layer_lock(ll),
                    Message::LayerToggleLock(index),
                );
                let checkmark: Element<'_, Message> = container(if is_active {
                    crate::ui::icons::tinted(crate::ui::icons::CHECK, 11.0, CHECK_COLOR)
                } else {
                    iced::widget::Space::new().width(0).into()
                })
                .width(Length::Fixed(14.0))
                .into();
                let label =
                    text(&info.name)
                        .size(11)
                        .color(if is_active { LABEL_ON } else { LABEL_OFF });

                // The swatch + label area selects the layer as active; the
                // icon buttons above handle their own toggles.
                let select = button(row![swatch, label].spacing(5).align_y(iced::Center))
                    .on_press(Message::RibbonLayerChanged(name))
                    .style(|_: &Theme, status| button::Style {
                        background: Some(Background::Color(match status {
                            button::Status::Hovered | button::Status::Pressed => ROW_HOVER,
                            _ => Color::TRANSPARENT,
                        })),
                        ..Default::default()
                    })
                    .width(Fill)
                    .padding([4, 4]);

                container(
                    row![checkmark, vis, freeze, lock, select]
                        .spacing(5)
                        .align_y(iced::Center),
                )
                .padding([0, 4])
                .into()
            })
            .collect();

        let panel = container(column(rows))
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(PANEL_BG)),
                border: Border {
                    color: PANEL_BORDER,
                    width: 1.0,
                    radius: 3.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fixed(220.0));

        let top_offset = 28.0 + TOOL_BAR_H;
        let groups = self.modules.get(self.active)?.ribbon_groups();
        let left_offset = compute_layer_combo_left(&groups);
        let positioned = container(panel)
            .align_left(Fill)
            .align_top(Fill)
            .padding(Padding {
                top: top_offset,
                left: left_offset,
                ..Default::default()
            })
            .width(Fill)
            .height(Fill);

        Some(
            mouse_area(positioned)
                .on_press(Message::CloseRibbonDropdown)
                .into(),
        )
    }

    /// Floating popup for an annotate-tab style combo (text / dimension /
    /// multileader / table style). Returns `None` when `open_id` is not a
    /// style combo. Built as an overlay — like the layer combo and the
    /// Draw-tab dropdowns — so the list grows to fit its entries instead of
    /// being clipped to the ribbon-row height. (#153)
    fn style_combo_overlay(&self, open_id: &str) -> Option<Element<'_, Message>> {
        let groups = self.modules.get(self.active)?.ribbon_groups();

        // Locate the open style combo; capture its style key + manager command.
        let mut found: Option<(crate::modules::StyleKey, Option<&'static str>)> = None;
        'outer: for group in &groups {
            for item in &group.tools {
                if let RibbonItem::StyleComboGroup {
                    style_key,
                    combo_id,
                    manager_cmd,
                    ..
                } = item
                {
                    if *combo_id == open_id {
                        found = Some((*style_key, *manager_cmd));
                        break 'outer;
                    }
                }
            }
        }
        let (style_key, manager_cmd) = found?;

        let ctx = StyleContext {
            text_style_names: self.text_style_names.clone(),
            active_text_style: self.active_text_style.clone(),
            dim_style_names: self.dim_style_names.clone(),
            active_dim_style: self.active_dim_style.clone(),
            mleader_style_names: self.mleader_style_names.clone(),
            active_mleader_style: self.active_mleader_style.clone(),
            table_style_names: self.table_style_names.clone(),
            active_table_style: self.active_table_style.clone(),
        };
        let active = ctx.active_for(style_key).to_string();

        let row_style = |_: &Theme, status: button::Status| button::Style {
            background: Some(Background::Color(match status {
                button::Status::Hovered | button::Status::Pressed => ROW_HOVER,
                _ => Color::TRANSPARENT,
            })),
            ..Default::default()
        };

        let mut rows: Vec<Element<Message>> = ctx
            .names_for(style_key)
            .iter()
            .map(|name| {
                let is_sel = name.as_str() == active.as_str();
                let n = name.clone();
                let checkmark: Element<Message> = container(if is_sel {
                    crate::ui::icons::tinted(crate::ui::icons::CHECK, 11.0, CHECK_COLOR)
                } else {
                    iced::widget::Space::new().width(0).into()
                })
                .width(Length::Fixed(14.0))
                .into();
                button(
                    row![
                        checkmark,
                        text(name.clone()).size(11).color(if is_sel {
                            LABEL_ON
                        } else {
                            LABEL_OFF
                        }),
                    ]
                    .spacing(4)
                    .align_y(iced::Center),
                )
                .on_press(Message::RibbonStyleChanged {
                    key: style_key,
                    name: n,
                })
                .style(row_style)
                .width(Fill)
                .padding([4, 10])
                .into()
            })
            .collect();

        if let Some(mgr) = manager_cmd {
            rows.push(
                button(text("Manage…").size(11).color(LABEL_ON))
                    .on_press(Message::Command(mgr.to_string()))
                    .style(row_style)
                    .width(Fill)
                    .padding([4, 10])
                    .into(),
            );
        }

        let panel = container(column(rows))
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(PANEL_BG)),
                border: Border {
                    color: PANEL_BORDER,
                    width: 1.0,
                    radius: 3.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fixed(LARGE_W * 2.3));

        let top_offset = 28.0 + TOOL_BAR_H;
        let left_offset = compute_dropdown_left(&groups, open_id);
        let positioned = container(panel)
            .align_left(Fill)
            .align_top(Fill)
            .padding(Padding {
                top: top_offset,
                left: left_offset,
                ..Default::default()
            })
            .width(Fill)
            .height(Fill);

        Some(
            mouse_area(positioned)
                .on_press(Message::CloseRibbonDropdown)
                .into(),
        )
    }

    fn prop_color_overlay(&self) -> Option<Element<'_, Message>> {
        let picker = crate::ui::color_select::color_list(
            crate::ui::color_select::ColorExtras {
                by_layer: true,
                by_block: true,
            },
            Message::RibbonColorChanged,
            Message::OpenColorWindow(crate::app::ColorPickTarget::Ribbon),
        );

        let panel = container(picker)
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(PANEL_BG)),
                border: Border {
                    color: PANEL_BORDER,
                    width: 1.0,
                    radius: 3.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fixed(200.0));

        let top_offset = 28.0 + TOOL_BAR_H;
        let groups = self.modules.get(self.active)?.ribbon_groups();
        let left_offset = compute_prop_combo_left(&groups, PROP_COLOR_ID);
        let positioned = container(panel)
            .align_left(Fill)
            .align_top(Fill)
            .padding(Padding {
                top: top_offset,
                left: left_offset,
                ..Default::default()
            })
            .width(Fill)
            .height(Fill);

        Some(
            mouse_area(positioned)
                .on_press(Message::CloseRibbonDropdown)
                .into(),
        )
    }

    fn prop_linetype_overlay(&self) -> Option<Element<'_, Message>> {
        let active_lt = &self.active_linetype;

        let mut items: Vec<LinetypeItem> = vec![
            LinetypeItem {
                name: "ByLayer".to_string(),
                art: String::new(),
            },
            LinetypeItem {
                name: "ByBlock".to_string(),
                art: String::new(),
            },
        ];
        for lt in &self.available_linetypes {
            if lt.name != "ByLayer" && lt.name != "ByBlock" {
                items.push(lt.clone());
            }
        }

        let rows: Vec<Element<Message>> = items
            .into_iter()
            .map(|lt| {
                let is_cur = lt.name == *active_lt;
                let check: Element<'_, Message> = container(if is_cur {
                    crate::ui::icons::tinted(crate::ui::icons::CHECK, 11.0, CHECK_COLOR)
                } else {
                    iced::widget::Space::new().width(0).into()
                })
                .width(Length::Fixed(14.0))
                .into();
                let name_col = text(lt.name.clone())
                    .size(11)
                    .color(if is_cur { LABEL_ON } else { LABEL_OFF })
                    .width(Length::Fixed(90.0));
                let art_col = text(lt.art.clone()).size(9).color(Color {
                    r: 0.55,
                    g: 0.55,
                    b: 0.55,
                    a: 1.0,
                });
                let name = lt.name.clone();
                button(
                    row![check, name_col, art_col]
                        .spacing(4)
                        .align_y(iced::Center),
                )
                .on_press(Message::RibbonLinetypeChanged(name))
                .style(|_: &Theme, status| button::Style {
                    background: Some(Background::Color(match status {
                        button::Status::Hovered | button::Status::Pressed => ROW_HOVER,
                        _ => Color::TRANSPARENT,
                    })),
                    ..Default::default()
                })
                .width(Fill)
                .padding([4, 6])
                .into()
            })
            .collect();

        let list = container(scrollable(column(rows)).height(Length::Fixed(200.0)))
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(PANEL_BG)),
                border: Border {
                    color: PANEL_BORDER,
                    width: 1.0,
                    radius: 3.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fixed(220.0));

        let top_offset = 28.0 + TOOL_BAR_H;
        let groups = self.modules.get(self.active)?.ribbon_groups();
        let left_offset = compute_prop_combo_left(&groups, PROP_LINETYPE_ID);
        let positioned = container(list)
            .align_left(Fill)
            .align_top(Fill)
            .padding(Padding {
                top: top_offset,
                left: left_offset,
                ..Default::default()
            })
            .width(Fill)
            .height(Fill);

        Some(
            mouse_area(positioned)
                .on_press(Message::CloseRibbonDropdown)
                .into(),
        )
    }

    fn prop_lw_overlay(&self) -> Option<Element<'_, Message>> {
        let active_lw = self.active_lineweight;
        let rows: Vec<Element<Message>> = lw_options()
            .into_iter()
            .map(|item| {
                let is_cur = item.0 == active_lw;
                let label = item.to_string();
                let check: Element<'_, Message> = container(if is_cur {
                    crate::ui::icons::tinted(crate::ui::icons::CHECK, 11.0, CHECK_COLOR)
                } else {
                    iced::widget::Space::new().width(0).into()
                })
                .width(Length::Fixed(14.0))
                .into();
                button(
                    row![
                        check,
                        text(label)
                            .size(11)
                            .color(if is_cur { LABEL_ON } else { LABEL_OFF })
                    ]
                    .spacing(5)
                    .align_y(iced::Center),
                )
                .on_press(Message::RibbonLineweightChanged(item.0))
                .style(|_: &Theme, status| button::Style {
                    background: Some(Background::Color(match status {
                        button::Status::Hovered | button::Status::Pressed => ROW_HOVER,
                        _ => Color::TRANSPARENT,
                    })),
                    ..Default::default()
                })
                .width(Fill)
                .padding([4, 8])
                .into()
            })
            .collect();

        self.prop_overlay_positioned(rows, PROP_LW_ID, 140.0)
    }

    fn prop_overlay_positioned<'a>(
        &'a self,
        rows: Vec<Element<'a, Message>>,
        dd_id: &str,
        width: f32,
    ) -> Option<Element<'a, Message>> {
        let panel = container(column(rows))
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(PANEL_BG)),
                border: Border {
                    color: PANEL_BORDER,
                    width: 1.0,
                    radius: 3.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fixed(width));

        let top_offset = 28.0 + TOOL_BAR_H;
        let groups = self.modules.get(self.active)?.ribbon_groups();
        let left_offset = compute_prop_combo_left(&groups, dd_id);
        let positioned = container(panel)
            .align_left(Fill)
            .align_top(Fill)
            .padding(Padding {
                top: top_offset,
                left: left_offset,
                ..Default::default()
            })
            .width(Fill)
            .height(Fill);

        Some(
            mouse_area(positioned)
                .on_press(Message::CloseRibbonDropdown)
                .into(),
        )
    }
}

impl Default for Ribbon {
    fn default() -> Self {
        Self::new()
    }
}
