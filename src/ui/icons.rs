//! Shared monochrome UI-chrome icons rendered from bundled SVGs.
//!
//! Dropdown carets and the undo/redo controls used to be drawn as Unicode
//! glyphs (`▾`, `▲`, `↶`, `↷`). Those depend on the active text font carrying
//! the glyph: on desktop the system fallback fonts supply them, but the web
//! build bundles only Fira Sans, which lacks them, so they rendered as empty
//! boxes. Drawing them from SVG instead makes the chrome font-independent.

use iced::widget::{container, svg, Space};
use iced::{Color, Element, Length, Theme};

const TRI_DOWN: &[u8] = include_bytes!("../../assets/icons/ui/tri_down.svg");
const TRI_UP: &[u8] = include_bytes!("../../assets/icons/ui/tri_up.svg");
const TRI_RIGHT: &[u8] = include_bytes!("../../assets/icons/ui/tri_right.svg");
const UNDO: &[u8] = include_bytes!("../../assets/icons/ui/undo.svg");
const REDO: &[u8] = include_bytes!("../../assets/icons/ui/redo.svg");

// OSNAP marker symbols. Rendered as SVG (not Unicode glyphs) so the snap menu
// shows the right shapes on the web build, whose bundled Fira Sans lacks the
// geometric glyphs and rendered them as tofu boxes. (#138)
const OSNAP_ENDPOINT: &[u8] = include_bytes!("../../assets/icons/osnap/endpoint.svg");
const OSNAP_MIDPOINT: &[u8] = include_bytes!("../../assets/icons/osnap/midpoint.svg");
const OSNAP_CENTER: &[u8] = include_bytes!("../../assets/icons/osnap/center.svg");
const OSNAP_NODE: &[u8] = include_bytes!("../../assets/icons/osnap/node.svg");
const OSNAP_QUADRANT: &[u8] = include_bytes!("../../assets/icons/osnap/quadrant.svg");
const OSNAP_INTERSECTION: &[u8] = include_bytes!("../../assets/icons/osnap/intersection.svg");
const OSNAP_EXTENSION: &[u8] = include_bytes!("../../assets/icons/osnap/extension.svg");
const OSNAP_INSERTION: &[u8] = include_bytes!("../../assets/icons/osnap/insertion.svg");
const OSNAP_PERPENDICULAR: &[u8] =
    include_bytes!("../../assets/icons/osnap/perpendicular.svg");
const OSNAP_TANGENT: &[u8] = include_bytes!("../../assets/icons/osnap/tangent.svg");
const OSNAP_NEAREST: &[u8] = include_bytes!("../../assets/icons/osnap/nearest.svg");
const OSNAP_APPARENT: &[u8] = include_bytes!("../../assets/icons/osnap/apparent.svg");
const OSNAP_PARALLEL: &[u8] = include_bytes!("../../assets/icons/osnap/parallel.svg");
const OSNAP_GRID: &[u8] = include_bytes!("../../assets/icons/osnap/grid.svg");

const LAY_ON: &[u8] = include_bytes!("../../assets/icons/layers/layon.svg");
const LAY_OFF: &[u8] = include_bytes!("../../assets/icons/layers/layoff.svg");
const LAY_FRZ: &[u8] = include_bytes!("../../assets/icons/layers/layfrz.svg");
const LAY_THW: &[u8] = include_bytes!("../../assets/icons/layers/laythw.svg");
const LAY_LCK: &[u8] = include_bytes!("../../assets/icons/layers/laylck.svg");
const LAY_ULK: &[u8] = include_bytes!("../../assets/icons/layers/layulk.svg");

// Monochrome chrome glyphs (replace Unicode glyphs in buttons / menus / toolbars).
// All are black-on-transparent; recolour them at the call site with [`tinted`].
pub const CHECK: &[u8] = include_bytes!("../../assets/icons/ui/check.svg");
pub const CLOSE: &[u8] = include_bytes!("../../assets/icons/ui/close.svg");
pub const PLUS: &[u8] = include_bytes!("../../assets/icons/ui/plus.svg");
pub const TRASH: &[u8] = include_bytes!("../../assets/icons/ui/trash.svg");
pub const MENU: &[u8] = include_bytes!("../../assets/icons/ui/menu.svg");
pub const BOLT: &[u8] = include_bytes!("../../assets/icons/ui/bolt.svg");
pub const MOVE: &[u8] = include_bytes!("../../assets/icons/ui/move.svg");
pub const SPLIT_V: &[u8] = include_bytes!("../../assets/icons/ui/split_v.svg");
pub const SPLIT_H: &[u8] = include_bytes!("../../assets/icons/ui/split_h.svg");
pub const GRID: &[u8] = include_bytes!("../../assets/icons/ui/grid.svg");
pub const SNAP: &[u8] = include_bytes!("../../assets/icons/ui/snap.svg");
pub const UP: &[u8] = include_bytes!("../../assets/icons/ui/up.svg");
pub const DOC_NEW: &[u8] = include_bytes!("../../assets/icons/ui/doc_new.svg");
pub const DOC: &[u8] = include_bytes!("../../assets/icons/ui/doc.svg");
pub const FOLDER: &[u8] = include_bytes!("../../assets/icons/ui/folder.svg");
pub const SAVE: &[u8] = include_bytes!("../../assets/icons/ui/save.svg");
pub const PRINT: &[u8] = include_bytes!("../../assets/icons/ui/print.svg");
pub const GEAR: &[u8] = include_bytes!("../../assets/icons/ui/gear.svg");
pub const HELP: &[u8] = include_bytes!("../../assets/icons/ui/help.svg");
pub const HEART: &[u8] = include_bytes!("../../assets/icons/ui/heart.svg");
pub const DOT: &[u8] = include_bytes!("../../assets/icons/ui/dot.svg");
pub const TRI_LEFT_B: &[u8] = include_bytes!("../../assets/icons/ui/tri_left.svg");
pub const ARROW_LONG_RIGHT: &[u8] = include_bytes!("../../assets/icons/ui/arrow_long_right.svg");

/// Render one of the bundled SVGs tinted to `color` at a square `size`.
pub fn tinted<'a, M: 'a>(bytes: &'static [u8], size: f32, color: Color) -> Element<'a, M> {
    svg(svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
        .style(move |_: &Theme, _| svg::Style { color: Some(color) })
        .into()
}

/// Backwards-compatible alias used by the caret/undo/redo helpers below.
fn icon<'a, M: 'a>(bytes: &'static [u8], size: f32, color: Color) -> Element<'a, M> {
    tinted(bytes, size, color)
}

/// A fixed-width (14 px) "current row" check column: a green-tintable check
/// when `active`, otherwise an empty spacer that preserves alignment. Used by
/// the many dropdown / popup list rows that mark the selected entry.
pub fn check_cell<'a, M: 'a>(active: bool, color: Color) -> Element<'a, M> {
    let inner: Element<'a, M> = if active {
        tinted(CHECK, 11.0, color)
    } else {
        Space::new().width(0).into()
    };
    container(inner).width(Length::Fixed(14.0)).into()
}

/// Render a bundled SVG at its native colours (no tint) at a square `size`.
pub fn raw<'a, M: 'a>(bytes: &'static [u8], size: f32) -> Element<'a, M> {
    svg(svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
        .into()
}

/// SVG bytes for an OSNAP mode's marker symbol, for the snap menu. (#138)
pub fn osnap(snap: crate::snap::SnapType) -> &'static [u8] {
    use crate::snap::SnapType as S;
    match snap {
        S::Endpoint => OSNAP_ENDPOINT,
        S::Midpoint => OSNAP_MIDPOINT,
        S::Center => OSNAP_CENTER,
        S::Node => OSNAP_NODE,
        S::Quadrant => OSNAP_QUADRANT,
        S::Intersection => OSNAP_INTERSECTION,
        S::Extension => OSNAP_EXTENSION,
        S::Insertion => OSNAP_INSERTION,
        S::Perpendicular => OSNAP_PERPENDICULAR,
        S::Tangent => OSNAP_TANGENT,
        S::Nearest => OSNAP_NEAREST,
        S::ApparentIntersection => OSNAP_APPARENT,
        S::Parallel => OSNAP_PARALLEL,
        S::Grid => OSNAP_GRID,
        // Not shown in the snap menu; fall back to a neutral marker.
        S::ObjectPick => OSNAP_NEAREST,
    }
}

/// Layer visibility icon bytes (on / off).
pub fn layer_visible(visible: bool) -> &'static [u8] {
    if visible {
        LAY_ON
    } else {
        LAY_OFF
    }
}

/// Layer freeze icon bytes (frozen / thawed).
pub fn layer_freeze(frozen: bool) -> &'static [u8] {
    if frozen {
        LAY_FRZ
    } else {
        LAY_THW
    }
}

/// Layer lock icon bytes (locked / unlocked).
pub fn layer_lock(locked: bool) -> &'static [u8] {
    if locked {
        LAY_LCK
    } else {
        LAY_ULK
    }
}

/// Downward dropdown caret (replaces `▾`).
pub fn arrow_down<'a, M: 'a>(size: f32, color: Color) -> Element<'a, M> {
    icon(TRI_DOWN, size, color)
}

/// Upward dropdown caret, shown when a dropdown is open (replaces `▲`).
pub fn arrow_up<'a, M: 'a>(size: f32, color: Color) -> Element<'a, M> {
    icon(TRI_UP, size, color)
}

/// Rightward caret for a collapsed item (replaces `▸`).
pub fn arrow_right<'a, M: 'a>(size: f32, color: Color) -> Element<'a, M> {
    icon(TRI_RIGHT, size, color)
}

/// Caret that flips up/down with `open`.
pub fn arrow_toggle<'a, M: 'a>(open: bool, size: f32, color: Color) -> Element<'a, M> {
    if open {
        arrow_up(size, color)
    } else {
        arrow_down(size, color)
    }
}

/// Undo curved arrow (replaces `↶`).
pub fn undo<'a, M: 'a>(size: f32, color: Color) -> Element<'a, M> {
    icon(UNDO, size, color)
}

/// Redo curved arrow (replaces `↷`).
pub fn redo<'a, M: 'a>(size: f32, color: Color) -> Element<'a, M> {
    icon(REDO, size, color)
}
