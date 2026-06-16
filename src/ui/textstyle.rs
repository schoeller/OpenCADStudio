//! Text Style Font Browser window — fills the entire OS window.

use crate::app::Message;
use crate::app::StyleKind;
use iced::widget::{button, checkbox, column, container, row, scrollable, text, text_input, Space};
use iced::{Background, Border, Color, Element, Fill, Theme};

/// View-model for the Text Style editor window.
pub struct TextStyleView<'a> {
    pub styles: Vec<String>,
    pub selected: &'a str,
    /// Name of the current text style (marked with ✓ in the list).
    pub current: &'a str,
    pub font_buf: &'a str,
    pub width_buf: &'a str,
    pub oblique_buf: &'a str,
    pub height_buf: &'a str,
    pub bigfont_buf: &'a str,
    pub ttf_buf: &'a str,
    pub backward: bool,
    pub upside_down: bool,
    pub annotative: bool,
    /// Name of the style being renamed inline (double-clicked), if any.
    pub rename_active: Option<&'a str>,
    /// Edit buffer for the inline rename text input.
    pub rename_buf: &'a str,
}

const BORDER: Color = Color {
    r: 0.35,
    g: 0.35,
    b: 0.35,
    a: 1.0,
};
const TEXT: Color = Color {
    r: 0.88,
    g: 0.88,
    b: 0.88,
    a: 1.0,
};
const DIM: Color = Color {
    r: 0.55,
    g: 0.55,
    b: 0.55,
    a: 1.0,
};
const ACCENT: Color = Color {
    r: 0.25,
    g: 0.50,
    b: 0.85,
    a: 1.0,
};
const ACTIVE: Color = Color {
    r: 0.20,
    g: 0.40,
    b: 0.70,
    a: 1.0,
};
const FIELD: Color = Color {
    r: 0.10,
    g: 0.10,
    b: 0.10,
    a: 1.0,
};
const LIST: Color = Color {
    r: 0.12,
    g: 0.12,
    b: 0.12,
    a: 1.0,
};

const BUILTIN_FONTS: &[&str] = &[
    "Standard", "ISO", "Simplex", "RomanS", "RomanD", "RomanC", "RomanT", "ItalicC", "ItalicT",
    "ScriptS", "ScriptC", "GothGBT", "GothGRT", "GothITT", "GreekC", "Symbol",
    "ISO3098", "Unicode",
];

fn list_item(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_: &Theme, st| button::Style {
        background: Some(Background::Color(match (active, st) {
            (true, _) => ACTIVE,
            (false, button::Status::Hovered | button::Status::Pressed) => Color {
                r: 0.26,
                g: 0.26,
                b: 0.26,
                a: 1.0,
            },
            _ => Color::TRANSPARENT,
        })),
        text_color: TEXT,
        ..Default::default()
    }
}

fn field_style(_: &Theme, _: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: Background::Color(FIELD),
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 3.0.into(),
        },
        icon: TEXT,
        placeholder: DIM,
        value: TEXT,
        selection: ACCENT,
    }
}

fn vsep<'a>() -> Element<'a, Message> {
    container(Space::new().width(1).height(Fill))
        .width(1)
        .height(Fill)
        .style(|_: &Theme| container::Style {
            background: Some(Background::Color(BORDER)),
            ..Default::default()
        })
        .into()
}

pub fn view_window<'a>(v: TextStyleView<'a>) -> Element<'a, Message> {
    let TextStyleView {
        styles,
        selected,
        current,
        font_buf,
        width_buf,
        oblique_buf,
        height_buf,
        bigfont_buf,
        ttf_buf,
        backward,
        upside_down,
        annotative,
        rename_active,
        rename_buf,
    } = v;
    // ── Middle: Font browser ──────────────────────────────────────────────
    let font_items: Vec<Element<'_, Message>> = BUILTIN_FONTS
        .iter()
        .map(|&f| {
            let is_sel = font_buf == f;
            button(text(f).size(10))
                .on_press(Message::TextStyleFontPick(f.to_string()))
                .style(list_item(is_sel))
                .padding([3, 8])
                .width(Fill)
                .into()
        })
        .collect();

    let font_panel = container(
        column![
            text("Font File").size(10).color(DIM),
            container(scrollable(column(font_items).spacing(1)).height(Fill))
                .style(|_: &Theme| container::Style {
                    background: Some(Background::Color(LIST)),
                    border: Border {
                        color: BORDER,
                        width: 1.0,
                        radius: 3.0.into()
                    },
                    ..Default::default()
                })
                .width(Fill)
                .height(Fill)
                .padding(2),
            text_input("font file…", font_buf)
                .on_input(|v| Message::TextStyleEdit {
                    field: "font",
                    value: v
                })
                .style(field_style)
                .size(11)
                .width(Fill),
        ]
        .spacing(6)
        .height(Fill),
    )
    .width(190)
    .height(Fill)
    .padding([12, 8]);

    // Labeled numeric/text field row → TextStyleEdit { field, value }.
    fn frow<'a>(
        label: &'a str,
        ph: &'a str,
        buf: &'a str,
        field: &'static str,
    ) -> Element<'a, Message> {
        row![
            text(label).size(11).color(DIM).width(120),
            text_input(ph, buf)
                .on_input(move |v| Message::TextStyleEdit { field, value: v })
                .style(field_style)
                .size(11)
                .width(140),
        ]
        .spacing(6)
        .align_y(iced::Center)
        .into()
    }

    // ── Right: Properties ─────────────────────────────────────────────────
    let props_panel = container(
        column![
            text("Properties").size(11).color(ACCENT),
            frow("Big Font:", "big-font file…", bigfont_buf, "bigfont"),
            frow("TrueType Font:", "e.g. Arial", ttf_buf, "ttf"),
            frow("Fixed Height:", "0 = variable", height_buf, "height"),
            frow("Width Factor:", "1.0", width_buf, "width"),
            frow("Oblique (°):", "0.0", oblique_buf, "oblique"),
            row![
                checkbox(backward)
                    .label("Backward")
                    .on_toggle(|_| Message::TextStyleToggle("backward"))
                    .size(15)
                    .text_size(11),
                checkbox(upside_down)
                    .label("Upside down")
                    .on_toggle(|_| Message::TextStyleToggle("upside_down"))
                    .size(15)
                    .text_size(11),
            ]
            .spacing(16),
            checkbox(annotative)
                .label("Annotative")
                .on_toggle(|_| Message::TextStyleToggle("annotative"))
                .size(15)
                .text_size(11),
            Space::new().height(8),
            text("Preview").size(10).color(DIM),
            container(text("AaBbCc 0123").size(20))
                .style(|_: &Theme| container::Style {
                    background: Some(Background::Color(FIELD)),
                    border: Border {
                        color: BORDER,
                        width: 1.0,
                        radius: 4.0.into()
                    },
                    ..Default::default()
                })
                .padding(12)
                .width(Fill),
        ]
        .spacing(10)
        .height(Fill),
    )
    .width(Fill)
    .height(Fill)
    .padding(iced::Padding {
        top: 12.0,
        right: 12.0,
        bottom: 12.0,
        left: 8.0,
    });

    // ── System TrueType fonts (read from the user's machine via fontdb) ──
    let ttf_items: Vec<Element<'_, Message>> = crate::scene::text::sysfont::families()
        .iter()
        .map(|fam| {
            let is_sel = ttf_buf.eq_ignore_ascii_case(fam);
            button(text(fam).size(10))
                .on_press(Message::TextStyleEdit {
                    field: "ttf",
                    value: fam.clone(),
                })
                .style(list_item(is_sel))
                .padding([3, 8])
                .width(Fill)
                .into()
        })
        .collect();

    let ttf_panel = container(
        column![
            text("TrueType (system)").size(10).color(DIM),
            container(scrollable(column(ttf_items).spacing(1)).height(Fill))
                .style(|_: &Theme| container::Style {
                    background: Some(Background::Color(LIST)),
                    border: Border {
                        color: BORDER,
                        width: 1.0,
                        radius: 3.0.into()
                    },
                    ..Default::default()
                })
                .width(Fill)
                .height(Fill)
                .padding(2),
            text_input("TrueType font…", ttf_buf)
                .on_input(|v| Message::TextStyleEdit {
                    field: "ttf",
                    value: v
                })
                .style(field_style)
                .size(11)
                .width(Fill),
        ]
        .spacing(6)
        .height(Fill),
    )
    .width(190)
    .height(Fill)
    .padding([12, 8]);

    let editor = row![font_panel, vsep(), ttf_panel, vsep(), props_panel].height(Fill);

    crate::ui::style_manager::view(crate::ui::style_manager::Scaffold {
        kind: StyleKind::Text,
        styles: &styles,
        selected,
        current: Some(current),
        rename_active,
        rename_buf,
        on_new: Message::TextStyleDialogNew,
        on_copy: Message::TextStyleDialogCopy,
        on_delete: Message::TextStyleDialogDelete,
        on_select: Message::TextStyleDialogSelect,
        on_set_current: Message::TextStyleDialogSetCurrent,
        on_apply: Message::TextStyleApply,
        editor: editor.into(),
    })
}
