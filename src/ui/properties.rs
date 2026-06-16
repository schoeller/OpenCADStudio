//! Properties panel — OpenCADStudio-style editable object properties.
//!
//! Shows two sections (General + Geometry) for the selected entity.
//! • Layer      → combo_box  (options from document layer table)
//! • Color      → inline color picker  (ByLayer / ByBlock / ACI palette)
//! • Lineweight → combo_box  (standard CAD lineweight list)
//! • Linetype   → read-only for now
//! • Geometry   → text_input per coordinate / dimension field

use rustc_hash::FxHashMap as HashMap;
use std::fmt;

use crate::ui::ROW_H;
use acadrust::types::{Color as AcadColor, LineWeight};
use acadrust::Handle;
use iced::widget::{button, column, combo_box, container, row, scrollable, text, text_input};
use iced::{Background, Border, Color, Element, Length, Padding, Theme};

// ── Row-height-derived constants ─────────────────────────────────────────
const FONT_SZ: f32 = ROW_H * 0.42; // ≈11 px
const COMBO_PAD_V: f32 = (ROW_H - FONT_SZ * 1.3 - 2.0) / 2.0; // fills combo to ROW_H
const SWATCH_SZ: f32 = ROW_H * 0.54; // ≈14 px color swatch

use crate::app::Message;
use crate::scene::model::object::{PropSection, PropValue};

const VARIES_LABEL: &str = "*VARIES*";

// ── Linetype item (name + ASCII art for combo_box) ───────────────────────

#[derive(Clone, PartialEq, Debug)]
pub struct LinetypeItem {
    pub name: String,
    pub art: String,
}

impl fmt::Display for LinetypeItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.art.is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{}  {}", self.name, self.art)
        }
    }
}

// ── Lineweight wrapper (needs ToString for combo_box) ─────────────────────

#[derive(Clone, PartialEq, Debug)]
pub struct LwItem(pub LineWeight);

impl fmt::Display for LwItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            LineWeight::ByLayer => write!(f, "ByLayer"),
            LineWeight::ByBlock => write!(f, "ByBlock"),
            LineWeight::Default => write!(f, "Default"),
            LineWeight::Value(v) => write!(f, "{:.2} mm", v as f64 / 100.0),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct SelectionGroup {
    pub label: String,
    pub handles: Vec<Handle>,
}

impl fmt::Display for SelectionGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label)
    }
}

/// All standard CAD lineweight options for the combobox.
pub fn lw_options() -> Vec<LwItem> {
    [
        LineWeight::ByLayer,
        LineWeight::ByBlock,
        LineWeight::Default,
        LineWeight::Value(0),
        LineWeight::Value(5),
        LineWeight::Value(9),
        LineWeight::Value(13),
        LineWeight::Value(15),
        LineWeight::Value(18),
        LineWeight::Value(20),
        LineWeight::Value(25),
        LineWeight::Value(30),
        LineWeight::Value(35),
        LineWeight::Value(40),
        LineWeight::Value(50),
        LineWeight::Value(53),
        LineWeight::Value(60),
        LineWeight::Value(70),
        LineWeight::Value(80),
        LineWeight::Value(90),
        LineWeight::Value(100),
        LineWeight::Value(106),
        LineWeight::Value(120),
        LineWeight::Value(140),
        LineWeight::Value(158),
        LineWeight::Value(200),
        LineWeight::Value(211),
    ]
    .iter()
    .copied()
    .map(LwItem)
    .collect()
}

// ── PropertiesPanel ───────────────────────────────────────────────────────

#[derive(Clone)]
pub struct PropertiesPanel {
    pub sections: Vec<PropSection>,
    pub title: String,
    pub selection_groups: Vec<SelectionGroup>,
    pub selected_group: Option<SelectionGroup>,
    /// Linetype items (name + ASCII art) from the document — used for combo_box options.
    pub linetype_items: Vec<LinetypeItem>,
    pub selection_group_combo: combo_box::State<SelectionGroup>,
    pub choice_combos: HashMap<String, combo_box::State<String>>,
    pub layer_combo: combo_box::State<String>,
    pub lineweight_combo: combo_box::State<LwItem>,
    pub linetype_combo: combo_box::State<LinetypeItem>,
    pub hatch_pattern_combo: combo_box::State<String>,
    /// In-progress text edits keyed by `field` name.
    pub edit_buf: HashMap<String, String>,
    /// Whether the quick color picker dropdown is open.
    pub color_picker_open: bool,
    /// Whether the full 16×16 ACI palette is expanded inside the color picker.
    pub color_palette_open: bool,
}

impl Default for PropertiesPanel {
    fn default() -> Self {
        Self {
            sections: vec![],
            title: String::new(),
            selection_groups: vec![],
            selected_group: None,
            linetype_items: vec![],
            selection_group_combo: combo_box::State::new(vec![]),
            choice_combos: HashMap::default(),
            layer_combo: combo_box::State::new(vec![]),
            lineweight_combo: combo_box::State::new(lw_options()),
            linetype_combo: combo_box::State::new(vec![]),
            hatch_pattern_combo: combo_box::State::new(crate::scene::model::hatch_patterns::names()),
            edit_buf: HashMap::default(),
            color_picker_open: false,
            color_palette_open: false,
        }
    }
}

impl PropertiesPanel {
    pub fn empty() -> Self {
        Self {
            title: "No Selection".into(),
            ..Default::default()
        }
    }

    pub fn selected_handles(&self) -> Vec<Handle> {
        self.selected_group
            .as_ref()
            .map(|group| group.handles.clone())
            .unwrap_or_default()
    }

    pub fn view(&self) -> Element<'_, Message> {
        // ── Header ──────────────────────────────────────────────────────────
        let header = container(text("Properties").size(12).color(Color::WHITE))
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(HEADER_BG)),
                ..Default::default()
            })
            .width(Length::Fill)
            .padding([6, 10]);

        // ── Title bar (entity type / "No Selection") ─────────────────────
        let title_content: Element<'_, Message> = if self.selection_groups.is_empty() {
            text(crate::ui::text_util::elide(&self.title, 34)).size(FONT_SZ).color(SECTION_LABEL).into()
        } else {
            combo_box(
                &self.selection_group_combo,
                "",
                self.selected_group.as_ref(),
                Message::PropSelectionGroupChanged,
            )
            .size(FONT_SZ)
            .padding([2, 6])
            .input_style(combo_input_style)
            .width(Length::Fill)
            .into()
        };

        let title_bar = container(title_content)
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(SECTION_BG)),
                border: Border {
                    color: BORDER,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fill)
            .padding([4, 10]);

        // ── Content ─────────────────────────────────────────────────────────
        let content: Element<'_, Message> = if self.sections.is_empty() {
            container(
                text("Select an object to view properties")
                    .size(10)
                    .color(HINT_COLOR),
            )
            .padding([10, 10])
            .into()
        } else {
            let mut col = column![].spacing(0);
            for section in &self.sections {
                col = col.push(self.render_section(section));
            }
            scrollable(col).into()
        };

        container(column![header, title_bar, content])
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(PANEL_BG)),
                border: Border {
                    color: BORDER,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .width(250)
            .height(Length::Fill)
            .into()
    }

    /// Compact floating panel for Quick Properties: the title plus the same
    /// editable section rows as the docked panel, sized to its content.
    /// Returns `None` when nothing is selected.
    pub fn quick_view(&self) -> Option<Element<'_, Message>> {
        if self.sections.is_empty() {
            return None;
        }
        let title = container(text(crate::ui::text_util::elide(&self.title, 34)).size(FONT_SZ).color(SECTION_LABEL))
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(SECTION_BG)),
                border: Border {
                    color: BORDER,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fill)
            .padding([4, 10]);

        let mut col = column![title].spacing(0);
        for section in &self.sections {
            col = col.push(self.render_section(section));
        }

        Some(
            container(col)
                .style(|_: &Theme| container::Style {
                    background: Some(Background::Color(PANEL_BG)),
                    border: Border {
                        color: BORDER,
                        width: 1.0,
                        radius: 3.0.into(),
                    },
                    ..Default::default()
                })
                .width(230)
                .into(),
        )
    }

    // ── Section renderer ──────────────────────────────────────────────────

    fn render_section<'a>(&'a self, section: &'a PropSection) -> Element<'a, Message> {
        // Section header
        let hdr = container(text(&section.title).size(10).color(Color::WHITE))
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(SECTION_HDR_BG)),
                border: Border {
                    color: BORDER,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fill)
            .padding([3, 8]);

        let mut col = column![hdr].spacing(0);

        for prop in &section.props {
            match &prop.value {
                PropValue::ColorChoice(color) => {
                    col = col.push(self.render_color_row(&prop.label, *color));
                }
                PropValue::ColorVaries => {
                    col = col.push(self.render_color_varies_row(&prop.label));
                }
                PropValue::LayerChoice(layer) => {
                    col = col.push(self.render_layer_row(&prop.label, layer));
                }
                PropValue::LwChoice(lw) => {
                    col = col.push(self.render_lw_row(&prop.label, *lw));
                }
                PropValue::LwVaries => {
                    col = col.push(self.render_lw_varies_row(&prop.label));
                }
                PropValue::LinetypeChoice(lt) => {
                    col = col.push(self.render_linetype_row(&prop.label, lt));
                }
                PropValue::Choice { selected, options } => {
                    col = col.push(self.render_choice_row(
                        &prop.label,
                        prop.field,
                        selected,
                        options,
                    ));
                }
                PropValue::BoolToggle { field, value } => {
                    col = col.push(render_bool_row(&prop.label, *field, *value));
                }
                PropValue::EditText(val) => {
                    col = col.push(self.render_edit_row(&prop.label, prop.field, val));
                }
                PropValue::ReadOnly(val) => {
                    col = col.push(render_ro_row(&prop.label, val));
                }
                PropValue::HatchPatternChoice(current) => {
                    col = col.push(self.render_hatch_pattern_row(&prop.label, current));
                }
            }
        }

        col.into()
    }

    // ── Layer row (combo_box) ─────────────────────────────────────────────

    fn render_layer_row<'a>(&'a self, label: &'a str, current: &'a str) -> Element<'a, Message> {
        let selected = if current == VARIES_LABEL {
            None
        } else {
            Some(current.to_string())
        };
        let combo = combo_box(
            &self.layer_combo,
            VARIES_LABEL,
            selected.as_ref(),
            Message::PropLayerChanged,
        )
        .size(FONT_SZ)
        .padding(Padding {
            top: COMBO_PAD_V,
            bottom: COMBO_PAD_V,
            left: 6.0,
            right: 6.0,
        })
        .input_style(combo_input_style)
        .width(Length::Fill);

        prop_row_widget(label, combo.into())
    }

    // ── Color row (custom picker) ─────────────────────────────────────────

    fn render_color_row<'a>(&'a self, label: &'a str, color: AcadColor) -> Element<'a, Message> {
        let selector = crate::ui::color_select::color_selector(
            color,
            self.color_picker_open,
            crate::ui::color_select::ColorExtras {
                by_layer: true,
                by_block: true,
            },
            Message::PropColorChanged,
            Message::PropColorPickerToggle,
            Message::OpenColorWindow(crate::app::ColorPickTarget::Properties),
        );
        prop_row_widget(label, selector)
    }

    fn render_color_varies_row<'a>(&'a self, label: &'a str) -> Element<'a, Message> {
        let color_btn = button(
            row![
                container(text("?").size(10).color(VALUE_COLOR))
                    .style(move |_: &Theme| container::Style {
                        background: Some(Background::Color(Color {
                            r: 0.32,
                            g: 0.32,
                            b: 0.32,
                            a: 1.0,
                        })),
                        border: Border {
                            color: Color {
                                r: 0.0,
                                g: 0.0,
                                b: 0.0,
                                a: 0.5
                            },
                            width: 1.0,
                            radius: 2.0.into()
                        },
                        ..Default::default()
                    })
                    .width(SWATCH_SZ)
                    .height(SWATCH_SZ)
                    .align_x(iced::Center)
                    .align_y(iced::Center),
                text(VARIES_LABEL).size(FONT_SZ).color(VALUE_COLOR),
            ]
            .spacing(4)
            .align_y(iced::Center),
        )
        .on_press(Message::PropColorPickerToggle)
        .style(combo_btn_style)
        .padding(Padding {
            top: COMBO_PAD_V,
            bottom: COMBO_PAD_V,
            left: 6.0,
            right: 6.0,
        })
        .width(Length::Fill);

        let color_row = prop_row_widget(label, color_btn.into());
        if self.color_picker_open {
            column![color_row, self.render_color_picker()]
                .spacing(0)
                .into()
        } else {
            color_row
        }
    }

    fn render_color_picker(&self) -> Element<'_, Message> {
        color_picker_dropdown(
            self.color_palette_open,
            Message::PropColorPaletteToggle,
            Some(Message::PropColorChanged(AcadColor::ByLayer)),
            Some(Message::PropColorChanged(AcadColor::ByBlock)),
            |aci| Message::PropColorChanged(AcadColor::Index(aci)),
        )
    }

    // ── Lineweight row (combo_box) ────────────────────────────────────────

    fn render_lw_row<'a>(&'a self, label: &'a str, lw: LineWeight) -> Element<'a, Message> {
        let selected = LwItem(lw);
        let combo = combo_box(
            &self.lineweight_combo,
            "",
            Some(&selected),
            |item: LwItem| Message::PropLwChanged(item.0),
        )
        .size(FONT_SZ)
        .padding(Padding {
            top: COMBO_PAD_V,
            bottom: COMBO_PAD_V,
            left: 6.0,
            right: 6.0,
        })
        .input_style(combo_input_style)
        .width(Length::Fill);

        prop_row_widget(label, combo.into())
    }

    fn render_lw_varies_row<'a>(&'a self, label: &'a str) -> Element<'a, Message> {
        let combo = combo_box(
            &self.lineweight_combo,
            VARIES_LABEL,
            None,
            |item: LwItem| Message::PropLwChanged(item.0),
        )
        .size(FONT_SZ)
        .padding(Padding {
            top: COMBO_PAD_V,
            bottom: COMBO_PAD_V,
            left: 6.0,
            right: 6.0,
        })
        .input_style(combo_input_style)
        .width(Length::Fill);

        prop_row_widget(label, combo.into())
    }

    // ── Linetype row (combo_box) ──────────────────────────────────────────

    fn render_linetype_row<'a>(&'a self, label: &'a str, current: &'a str) -> Element<'a, Message> {
        // Normalise: empty string = "ByLayer"
        let display = if current.is_empty() {
            "ByLayer"
        } else {
            current
        };
        let selected = self
            .linetype_items
            .iter()
            .find(|item| item.name.eq_ignore_ascii_case(display))
            .cloned();
        let combo = combo_box(
            &self.linetype_combo,
            VARIES_LABEL,
            selected.as_ref(),
            |item: LinetypeItem| Message::PropLinetypeChanged(item.name),
        )
        .size(FONT_SZ)
        .padding(Padding {
            top: COMBO_PAD_V,
            bottom: COMBO_PAD_V,
            left: 6.0,
            right: 6.0,
        })
        .input_style(combo_input_style)
        .width(Length::Fill);

        prop_row_widget(label, combo.into())
    }

    fn render_choice_row<'a>(
        &'a self,
        label: &'a str,
        field: &'static str,
        current: &'a str,
        _options: &'a [String],
    ) -> Element<'a, Message> {
        let Some(state) = self.choice_combos.get(field) else {
            return render_ro_row(label, current);
        };

        let selected = if current == VARIES_LABEL {
            None
        } else {
            Some(current.to_string())
        };
        let combo = combo_box(state, VARIES_LABEL, selected.as_ref(), move |value| {
            Message::PropGeomChoiceChanged { field, value }
        })
        .size(FONT_SZ)
        .padding(Padding {
            top: COMBO_PAD_V,
            bottom: COMBO_PAD_V,
            left: 6.0,
            right: 6.0,
        })
        .input_style(combo_input_style)
        .width(Length::Fill);

        prop_row_widget(label, combo.into())
    }

    // ── Editable geometry row (text_input) ────────────────────────────────

    fn render_edit_row<'a>(
        &'a self,
        label: &'a str,
        field: &'static str,
        entity_val: &'a str,
    ) -> Element<'a, Message> {
        let display = self
            .edit_buf
            .get(field)
            .map(|s| s.as_str())
            .unwrap_or(entity_val);

        let ti = text_input("", display)
            .on_input(move |v| Message::PropGeomInput { field, value: v })
            .on_submit(Message::PropGeomCommit(field))
            .size(FONT_SZ)
            .style(text_input_style)
            .padding([3, 6])
            .width(Length::Fill);

        prop_row_widget(label, ti.into())
    }

    fn render_hatch_pattern_row<'a>(
        &'a self,
        label: &'a str,
        current: &'a str,
    ) -> Element<'a, Message> {
        let selected = if current == VARIES_LABEL {
            None
        } else {
            Some(current.to_string())
        };
        let combo = combo_box(
            &self.hatch_pattern_combo,
            VARIES_LABEL,
            selected.as_ref(),
            Message::PropHatchPatternChanged,
        )
        .size(FONT_SZ)
        .padding(Padding {
            top: COMBO_PAD_V,
            bottom: COMBO_PAD_V,
            left: 6.0,
            right: 6.0,
        })
        .input_style(combo_input_style)
        .width(Length::Fill);

        prop_row_widget(label, combo.into())
    }
}

// ── Shared color picker widget ────────────────────────────────────────────

/// Builds the color picker dropdown content (standard swatches + optional
/// ByLayer/ByBlock + "More Colors…" expanding to full ACI palette).
/// Use this from both the Properties panel and the Layer Manager.
pub fn color_picker_dropdown<'a>(
    palette_open: bool,
    palette_toggle_msg: Message,
    by_layer_msg: Option<Message>,
    by_block_msg: Option<Message>,
    on_aci: impl Fn(u8) -> Message + 'a,
) -> Element<'a, Message> {
    // ByLayer / ByBlock row (optional)
    let extras: Option<Element<'a, Message>> = match (by_layer_msg, by_block_msg) {
        (Some(bl), Some(bb)) => Some(
            row![
                picker_text_btn("ByLayer", bl),
                picker_text_btn("ByBlock", bb)
            ]
            .spacing(4)
            .into(),
        ),
        (Some(bl), None) => Some(picker_text_btn("ByLayer", bl)),
        (None, Some(bb)) => Some(picker_text_btn("ByBlock", bb)),
        (None, None) => None,
    };

    // 9 standard ACI swatches (1-9)
    let standard: Element<'a, Message> = (1u8..=9u8)
        .fold(row![].spacing(2), |r, idx| {
            let c = AcadColor::Index(idx);
            let (bg, _) = acad_color_display(c);
            let msg = on_aci(idx);
            r.push(
                button(text("").width(18).height(18))
                    .on_press(msg)
                    .style(move |_: &Theme, status| button::Style {
                        background: Some(Background::Color(bg)),
                        border: Border {
                            color: if matches!(status, button::Status::Hovered) {
                                Color::WHITE
                            } else {
                                Color::BLACK
                            },
                            width: if matches!(status, button::Status::Hovered) {
                                1.5
                            } else {
                                1.0
                            },
                            radius: 2.0.into(),
                        },
                        ..Default::default()
                    })
                    .padding(0),
            )
        })
        .into();

    // "More Colors…" toggle button
    let more_btn = button(
        text(if palette_open {
            "▲ Less"
        } else {
            "▼ More Colors…"
        })
        .size(10)
        .color(HINT_COLOR),
    )
    .on_press(palette_toggle_msg)
    .style(|_: &Theme, _| button::Style {
        background: Some(Background::Color(PICKER_BG)),
        text_color: HINT_COLOR,
        ..Default::default()
    })
    .padding([2, 6])
    .width(Length::Fill);

    let inner = if let Some(e) = extras {
        column![e, standard, more_btn].spacing(4)
    } else {
        column![standard, more_btn].spacing(4)
    };

    let mut col = column![container(inner)
        .style(|_: &Theme| container::Style {
            background: Some(Background::Color(PICKER_BG)),
            border: Border {
                color: BORDER,
                width: 1.0,
                radius: 0.0.into()
            },
            ..Default::default()
        })
        .padding([6, 8])
        .width(Length::Fill)]
    .spacing(0);

    // Full ACI palette (expanded)
    if palette_open {
        const COLS: u16 = 16;
        let mut rows = column![].spacing(1);
        let mut idx: u16 = 1;
        while idx <= 255 {
            let mut r = row![].spacing(1);
            for _ in 0..COLS {
                if idx > 255 {
                    break;
                }
                let ci = idx as u8;
                let (bg, _) = acad_color_display(AcadColor::Index(ci));
                let msg = on_aci(ci);
                r = r.push(
                    button(text("").width(12).height(12))
                        .on_press(msg)
                        .style(move |_: &Theme, status| button::Style {
                            background: Some(Background::Color(bg)),
                            border: Border {
                                color: if matches!(status, button::Status::Hovered) {
                                    Color::WHITE
                                } else {
                                    Color {
                                        r: 0.0,
                                        g: 0.0,
                                        b: 0.0,
                                        a: 0.4,
                                    }
                                },
                                width: if matches!(status, button::Status::Hovered) {
                                    1.5
                                } else {
                                    1.0
                                },
                                radius: 1.0.into(),
                            },
                            ..Default::default()
                        })
                        .padding(0),
                );
                idx += 1;
            }
            rows = rows.push(r);
        }
        col = col.push(
            container(scrollable(rows).height(160))
                .style(|_: &Theme| container::Style {
                    background: Some(Background::Color(PICKER_BG)),
                    border: Border {
                        color: BORDER,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..Default::default()
                })
                .padding([4, 6])
                .width(Length::Fill),
        );
    }

    col.into()
}

// ── Standalone helpers ────────────────────────────────────────────────────

/// A boolean toggle button row (for "Invisible" etc.).
fn render_bool_row<'a>(label: &'a str, field: &'static str, value: bool) -> Element<'a, Message> {
    let btn_label = if value { "Yes" } else { "No" };
    let btn =
        button(
            text(btn_label)
                .size(FONT_SZ)
                .color(if value { WARN_COLOR } else { VALUE_COLOR }),
        )
        .on_press(Message::PropBoolToggle(field))
        .style(move |_: &Theme, status| {
            let bg = match status {
                button::Status::Hovered | button::Status::Pressed => HOVER_BG,
                _ => VALUE_BG,
            };
            button::Style {
                background: Some(Background::Color(bg)),
                border: Border {
                    color: BORDER,
                    width: 1.0,
                    radius: 2.0.into(),
                },
                text_color: if value { WARN_COLOR } else { VALUE_COLOR },
                ..Default::default()
            }
        })
        .padding([2, 6])
        .width(Length::Fill);

    prop_row_widget(label, btn.into())
}

fn render_ro_row<'a>(label: &'a str, value: &'a str) -> Element<'a, Message> {
    let value = crate::ui::text_util::elide(value, 22);
    let label_col = container(text(crate::ui::text_util::elide(label, 18)).size(FONT_SZ).color(LABEL_COLOR))
        .style(|_: &Theme| container::Style {
            background: Some(Background::Color(LABEL_BG)),
            ..Default::default()
        })
        .width(Length::FillPortion(5))
        .height(Length::Fixed(ROW_H))
        .align_y(iced::Center)
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 6.0,
            right: 6.0,
        });
    let value_col = container(text(value).size(FONT_SZ).color(VALUE_COLOR))
        .style(|_: &Theme| container::Style {
            background: Some(Background::Color(VALUE_BG)),
            ..Default::default()
        })
        .width(Length::FillPortion(6))
        .height(Length::Fixed(ROW_H))
        .align_y(iced::Center)
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 6.0,
            right: 6.0,
        });
    container(row![label_col, value_col])
        .height(Length::Fixed(ROW_H))
        .style(|_: &Theme| container::Style {
            border: Border {
                color: BORDER,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Build a label | widget property row.
fn prop_row_widget<'a>(label: &'a str, widget: Element<'a, Message>) -> Element<'a, Message> {
    let label_col = container(text(crate::ui::text_util::elide(label, 18)).size(FONT_SZ).color(LABEL_COLOR))
        .style(|_: &Theme| container::Style {
            background: Some(Background::Color(LABEL_BG)),
            ..Default::default()
        })
        .width(Length::FillPortion(5))
        .height(Length::Fixed(ROW_H))
        .align_y(iced::Center)
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 6.0,
            right: 6.0,
        });
    let value_col = container(widget)
        .style(|_: &Theme| container::Style {
            background: Some(Background::Color(VALUE_BG)),
            ..Default::default()
        })
        .width(Length::FillPortion(6))
        .height(Length::Fixed(ROW_H))
        .align_y(iced::Center)
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 2.0,
            right: 2.0,
        });
    container(row![label_col, value_col])
        .height(Length::Fixed(ROW_H))
        .style(|_: &Theme| container::Style {
            border: Border {
                color: BORDER,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// A plain text button used inside the color picker for ByLayer / ByBlock.
fn picker_text_btn(label: &str, msg: Message) -> Element<'_, Message> {
    button(text(label).size(FONT_SZ).color(VALUE_COLOR))
        .on_press(msg)
        .style(|_: &Theme, _| button::Style {
            background: Some(Background::Color(LABEL_BG)),
            border: Border {
                color: BORDER,
                width: 1.0,
                radius: 2.0.into(),
            },
            text_color: VALUE_COLOR,
            ..Default::default()
        })
        .padding([2, 8])
        .into()
}

// ── Color display helper ──────────────────────────────────────────────────

/// Returns an (iced::Color swatch_bg, display_label) pair for an AcadColor.
pub fn acad_color_display(c: AcadColor) -> (Color, &'static str) {
    match c {
        AcadColor::ByLayer => (
            Color {
                r: 0.35,
                g: 0.35,
                b: 0.35,
                a: 1.0,
            },
            "ByLayer",
        ),
        AcadColor::ByBlock => (
            Color {
                r: 0.25,
                g: 0.25,
                b: 0.45,
                a: 1.0,
            },
            "ByBlock",
        ),
        AcadColor::Index(i) => {
            let (r, g, b) = acadrust::types::aci_table::aci_to_rgb(i).unwrap_or((200, 200, 200));
            (
                Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0),
                aci_label(i),
            )
        }
        AcadColor::Rgb { r, g, b } => (
            Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0),
            "Custom",
        ),
    }
}

fn aci_label(idx: u8) -> &'static str {
    match idx {
        1 => "Red",
        2 => "Yellow",
        3 => "Green",
        4 => "Cyan",
        5 => "Blue",
        6 => "Magenta",
        7 => "White",
        8 => "Dark Gray",
        9 => "Light Gray",
        _ => "Index",
    }
}

// ── Widget style helpers ──────────────────────────────────────────────────

fn combo_btn_style(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => HOVER_BG,
        _ => VALUE_BG,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 2.0.into(),
        },
        text_color: VALUE_COLOR,
        ..Default::default()
    }
}

fn text_input_style(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let border_color = match status {
        text_input::Status::Focused { .. } => Color {
            r: 0.3,
            g: 0.6,
            b: 1.0,
            a: 1.0,
        },
        _ => BORDER,
    };
    text_input::Style {
        background: Background::Color(VALUE_BG),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: 2.0.into(),
        },
        icon: Color::TRANSPARENT,
        placeholder: HINT_COLOR,
        value: VALUE_COLOR,
        selection: Color {
            r: 0.2,
            g: 0.4,
            b: 0.8,
            a: 0.5,
        },
    }
}

fn combo_input_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    text_input_style(theme, status)
}

// ── Colour constants ──────────────────────────────────────────────────────

const PANEL_BG: Color = Color {
    r: 0.19,
    g: 0.19,
    b: 0.19,
    a: 1.0,
};
const HEADER_BG: Color = Color {
    r: 0.24,
    g: 0.24,
    b: 0.24,
    a: 1.0,
};
const SECTION_BG: Color = Color {
    r: 0.21,
    g: 0.21,
    b: 0.21,
    a: 1.0,
};
const SECTION_HDR_BG: Color = Color {
    r: 0.26,
    g: 0.26,
    b: 0.28,
    a: 1.0,
};
const LABEL_BG: Color = Color {
    r: 0.22,
    g: 0.22,
    b: 0.22,
    a: 1.0,
};
const VALUE_BG: Color = Color {
    r: 0.18,
    g: 0.18,
    b: 0.18,
    a: 1.0,
};
const HOVER_BG: Color = Color {
    r: 0.25,
    g: 0.25,
    b: 0.28,
    a: 1.0,
};
const PICKER_BG: Color = Color {
    r: 0.16,
    g: 0.16,
    b: 0.18,
    a: 1.0,
};
const LABEL_COLOR: Color = Color {
    r: 0.70,
    g: 0.70,
    b: 0.70,
    a: 1.0,
};
const VALUE_COLOR: Color = Color {
    r: 0.90,
    g: 0.90,
    b: 0.90,
    a: 1.0,
};
const HINT_COLOR: Color = Color {
    r: 0.45,
    g: 0.45,
    b: 0.50,
    a: 1.0,
};
const SECTION_LABEL: Color = Color {
    r: 0.75,
    g: 0.75,
    b: 0.75,
    a: 1.0,
};
const BORDER: Color = Color {
    r: 0.32,
    g: 0.32,
    b: 0.32,
    a: 1.0,
};
const WARN_COLOR: Color = Color {
    r: 1.00,
    g: 0.60,
    b: 0.10,
    a: 1.0,
};
