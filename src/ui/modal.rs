//! Shared in-canvas modal overlay.
//!
//! Former pop-up *windows* (layer manager, style editors, About, …) render as
//! centered overlays on top of the main view instead of separate OS windows.
//! The native build has one main window and the web build has only the canvas,
//! so both stack dialogs here — one code path for every platform.

use crate::app::Message;
use iced::widget::{button, column, container, mouse_area, opaque, row, stack, text};
use iced::{Background, Border, Color, Element, Length, Padding, Theme, Vector};

const PANEL: Color = Color {
    r: 0.13,
    g: 0.13,
    b: 0.13,
    a: 1.0,
};
const BORDER_C: Color = Color {
    r: 0.35,
    g: 0.35,
    b: 0.35,
    a: 1.0,
};
/// Title-bar background.
const TITLE_C: Color = Color {
    r: 0.18,
    g: 0.18,
    b: 0.18,
    a: 1.0,
};
/// Drag-handle arrow — bright green so the move affordance stands out.
const GRIP_C: Color = Color {
    r: 0.2,
    g: 1.0,
    b: 0.3,
    a: 1.0,
};

/// Stack `content` over `base` behind a dimmed backdrop, framed with a
/// draggable title bar (the ✕ close button at its right end). The backdrop only
/// dims and blocks clicks from reaching the view beneath — it does **not**
/// dismiss the dialog; closing is the ✕ button alone (`on_close`).
///
/// `offset` shifts the dialog from screen-centre so it can be dragged by its
/// title bar; pass `Vector::ZERO` to keep it centred.
pub fn modal<'a>(
    base: impl Into<Element<'a, Message>>,
    content: impl Into<Element<'a, Message>>,
    on_close: Message,
    offset: Vector,
) -> Element<'a, Message> {
    let close = button(text("✕").size(15))
        .on_press(on_close)
        .padding([1, 7])
        .style(close_style);

    // Draggable title bar: a grip handle next to the ✕. Kept `Shrink` (no
    // `Fill`) so a single Fill child can't blow the dialog out to the full
    // screen width — the dialog stays sized to its content. Pressing the grip
    // starts a drag (handled in `update`).
    let grip = mouse_area(
        container(text("✥").size(15).color(GRIP_C))
            .padding([1, 7])
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(TITLE_C)),
                border: Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
    )
    .on_press(Message::ModalGrab)
    .interaction(iced::mouse::Interaction::Grab);

    let title_bar = row![grip, close].spacing(6).align_y(iced::Center);

    // The column shrinks to the content's width; the title bar sits top-right
    // above it, mirroring the former ✕ placement with a drag handle.
    let framed = container(
        column![title_bar, content.into()]
            .spacing(6)
            .align_x(iced::alignment::Horizontal::Right),
    )
        .padding(10)
        .style(|_: &Theme| container::Style {
            background: Some(Background::Color(PANEL)),
            border: Border {
                color: BORDER_C,
                width: 1.0,
                radius: 6.0.into(),
            },
            ..Default::default()
        });

    // Position via asymmetric padding (padding is non-negative): shifting a
    // centred box by `d` on an axis needs (near − far) padding = 2·d there.
    let pad = Padding {
        top: offset.y.max(0.0) * 2.0,
        right: (-offset.x).max(0.0) * 2.0,
        bottom: (-offset.y).max(0.0) * 2.0,
        left: offset.x.max(0.0) * 2.0,
    };

    // The backdrop fills the screen so dragging keeps tracking the cursor even
    // when it leaves the title bar; release anywhere ends the drag.
    let backdrop = mouse_area(
        container(framed)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .padding(pad)
            .style(|_: &Theme| container::Style {
                background: Some(Background::Color(Color {
                    a: 0.55,
                    ..Color::BLACK
                })),
                ..Default::default()
            }),
    )
    .on_move(Message::ModalDragMove)
    .on_release(Message::ModalDragRelease);

    stack![
        base.into(),
        // `opaque` blocks pointer events from passing through, so the dimmed
        // backdrop swallows clicks instead of closing or hitting the view.
        opaque(backdrop),
    ]
    .into()
}

fn close_style(_: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => Color {
            r: 0.7,
            g: 0.2,
            b: 0.2,
            a: 1.0,
        },
        _ => Color {
            r: 0.25,
            g: 0.25,
            b: 0.25,
            a: 1.0,
        },
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::WHITE,
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}
