//! Adaptive two-block bar that wraps its trailing block onto a second row when
//! the width can't hold both blocks side by side. Used by the ribbon tab row
//! (lead = quick-access + undo/redo, trail = tabs) and the status bar
//! (lead = menu, trail = status pills, with the layout-tab strip as the Fill
//! `middle` filler between them).
//!
//! Layout, per row:
//!   • one row  → `lead` at the left; `trail` packed after it (`justify_end`
//!                false) or flush to the right edge (`justify_end` true);
//!                an optional Fill `middle` stretches through the gap.
//!   • two rows → `lead` (+ `middle` filling the rest) on row 1; `trail` on
//!                row 2, left- or right-aligned per `justify_end`.
//!
//! The measured total height is written to `height_out` (if set) so callers can
//! anchor overlays below the possibly-taller bar.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use rustc_hash::FxHashMap;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Widget};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Shell};
use iced::{Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector};

use crate::app::Message;

thread_local! {
    /// Screen bounds of every ribbon dropdown button, keyed by dropdown id, so
    /// an open dropdown's overlay can anchor directly below its widget at any
    /// size. Written by `PosReport` on draw, read by `dropdown_bounds`.
    static DD_BOUNDS: RefCell<FxHashMap<String, Rectangle>> =
        RefCell::new(FxHashMap::default());
}

/// Last-drawn screen bounds of the dropdown button with this id.
pub fn dropdown_bounds(id: &str) -> Option<Rectangle> {
    DD_BOUNDS.with(|m| m.borrow().get(id).copied())
}

pub struct WrapBar<'a> {
    lead: Element<'a, Message>,
    /// Optional Fill filler occupying the gap between `lead` and `trail`.
    middle: Option<Element<'a, Message>>,
    trail: Element<'a, Message>,
    /// Gap between adjacent blocks on a shared row.
    spacing: f32,
    /// Minimum height of a single row.
    min_row_h: f32,
    /// When true, `trail` sits at the right edge (justified); otherwise it is
    /// packed immediately after `lead`.
    justify_end: bool,
    /// Receives the measured total height (bits of an `f32`).
    height_out: Option<Arc<AtomicU32>>,
}

impl<'a> WrapBar<'a> {
    pub fn new(lead: Element<'a, Message>, trail: Element<'a, Message>) -> Self {
        Self {
            lead,
            middle: None,
            trail,
            spacing: 0.0,
            min_row_h: 28.0,
            justify_end: false,
            height_out: None,
        }
    }

    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    pub fn min_row_h(mut self, h: f32) -> Self {
        self.min_row_h = h;
        self
    }

    pub fn justify_end(mut self, justify: bool) -> Self {
        self.justify_end = justify;
        self
    }

    pub fn middle(mut self, middle: Element<'a, Message>) -> Self {
        self.middle = Some(middle);
        self
    }

    pub fn report_height(mut self, out: Arc<AtomicU32>) -> Self {
        self.height_out = Some(out);
        self
    }

    /// Elements in row order: lead, [middle], trail.
    fn refs(&self) -> Vec<&Element<'a, Message>> {
        let mut v = Vec::with_capacity(3);
        v.push(&self.lead);
        if let Some(m) = &self.middle {
            v.push(m);
        }
        v.push(&self.trail);
        v
    }

    fn refs_mut(&mut self) -> Vec<&mut Element<'a, Message>> {
        let mut v = Vec::with_capacity(3);
        v.push(&mut self.lead);
        if let Some(m) = &mut self.middle {
            v.push(m);
        }
        v.push(&mut self.trail);
        v
    }
}

impl<'a> Widget<Message, Theme, Renderer> for WrapBar<'a> {
    fn children(&self) -> Vec<widget::Tree> {
        self.refs().iter().map(|e| widget::Tree::new(*e)).collect()
    }

    fn diff(&self, tree: &mut widget::Tree) {
        let refs: Vec<_> = self.refs().iter().map(|e| e.as_widget()).collect();
        tree.diff_children(&refs);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let max = limits.max();
        // Measure blocks at their natural (unwrapped) width so the fit decision
        // and one-row placement use true content widths. A flex-wrap `trail`
        // (WrapFlow) only wraps when it is later laid out with a bounded width.
        let natural =
            layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, f32::INFINITY));

        let has_middle = self.middle.is_some();
        let trail_idx = if has_middle { 2 } else { 1 };

        let mut lead_node =
            self.lead
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, &natural);
        let mut trail_node = self.trail.as_widget_mut().layout(
            &mut tree.children[trail_idx],
            renderer,
            &natural,
        );

        let mut lead_sz = lead_node.size();
        let mut trail_sz = trail_node.size();

        let width = if max.width.is_finite() {
            max.width
        } else {
            lead_sz.width + self.spacing + trail_sz.width
        };

        let fits = max.width.is_finite()
            && lead_sz.width + self.spacing + trail_sz.width <= max.width;

        let row_h = lead_sz.height.max(trail_sz.height).max(self.min_row_h);
        let bounded = layout::Limits::new(Size::ZERO, Size::new(width, f32::INFINITY));

        let (lead_pos, trail_pos, middle_x, middle_gap, total_h);
        if fits {
            // One row: lead left, trail packed after it or flush right.
            let trail_x = if self.justify_end {
                (width - trail_sz.width).max(lead_sz.width + self.spacing)
            } else {
                lead_sz.width + self.spacing
            };
            lead_pos = Point::new(0.0, (row_h - lead_sz.height) / 2.0);
            trail_pos = Point::new(trail_x, (row_h - trail_sz.height) / 2.0);
            middle_x = lead_sz.width + self.spacing;
            middle_gap = (trail_x - self.spacing - middle_x).max(0.0);
            total_h = row_h;
        } else if has_middle {
            // 3-slot justified bar: lead on row 1, middle fills the rest of it,
            // trail (flex-wrap) drops onto the row(s) below.
            lead_pos = Point::new(0.0, (row_h - lead_sz.height) / 2.0);
            middle_x = lead_sz.width + self.spacing;
            middle_gap = (width - middle_x).max(0.0);

            trail_node = self.trail.as_widget_mut().layout(
                &mut tree.children[trail_idx],
                renderer,
                &bounded,
            );
            trail_sz = trail_node.size();
            trail_pos = Point::new(0.0, row_h);
            total_h = row_h + trail_sz.height;
        } else {
            // 2-slot dual-wrap: lead and trail each wrap within their OWN row
            // band, stacked so a wrapped lead item never lands on a trail row.
            lead_node =
                self.lead
                    .as_widget_mut()
                    .layout(&mut tree.children[0], renderer, &bounded);
            lead_sz = lead_node.size();
            let lead_h = lead_sz.height.max(self.min_row_h);

            trail_node = self.trail.as_widget_mut().layout(
                &mut tree.children[trail_idx],
                renderer,
                &bounded,
            );
            trail_sz = trail_node.size();
            let trail_h = trail_sz.height.max(self.min_row_h);

            lead_pos = Point::new(0.0, (lead_h - lead_sz.height) / 2.0);
            trail_pos = Point::new(0.0, lead_h + (trail_h - trail_sz.height) / 2.0);
            middle_x = 0.0;
            middle_gap = 0.0;
            total_h = lead_h + trail_h;
        }

        let mut children: Vec<layout::Node> = Vec::with_capacity(3);
        children.push(lead_node.move_to(lead_pos));

        if has_middle {
            let mid_limits =
                layout::Limits::new(Size::new(middle_gap, 0.0), Size::new(middle_gap, row_h));
            let mut mid_node = self.middle.as_mut().unwrap().as_widget_mut().layout(
                &mut tree.children[1],
                renderer,
                &mid_limits,
            );
            let mid_y = ((row_h - mid_node.size().height) / 2.0).max(0.0);
            mid_node = mid_node.move_to(Point::new(middle_x, mid_y));
            children.push(mid_node);
        }

        children.push(trail_node.move_to(trail_pos));

        if let Some(out) = &self.height_out {
            out.store(total_h.to_bits(), Ordering::Relaxed);
        }

        layout::Node::with_children(Size::new(width, total_h), children)
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for ((child, state), child_layout) in self
            .refs_mut()
            .into_iter()
            .zip(tree.children.iter_mut())
            .zip(layout.children())
        {
            child.as_widget_mut().update(
                state,
                event,
                child_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let mut interaction = mouse::Interaction::default();
        for ((child, state), child_layout) in self
            .refs()
            .into_iter()
            .zip(tree.children.iter())
            .zip(layout.children())
        {
            let i = child.as_widget().mouse_interaction(
                state,
                child_layout,
                cursor,
                viewport,
                renderer,
            );
            if i != mouse::Interaction::default() {
                interaction = i;
            }
        }
        interaction
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        for ((child, state), child_layout) in self
            .refs_mut()
            .into_iter()
            .zip(tree.children.iter_mut())
            .zip(layout.children())
        {
            child
                .as_widget_mut()
                .operate(state, child_layout, renderer, operation);
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for ((child, state), child_layout) in self
            .refs()
            .into_iter()
            .zip(tree.children.iter())
            .zip(layout.children())
        {
            child.as_widget().draw(
                state,
                renderer,
                theme,
                style,
                child_layout,
                cursor,
                viewport,
            );
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        // At most one child (the hovered tooltip, if any) yields an overlay;
        // return the first. Borrow fields and tree slots disjointly so the
        // returned overlay can outlive this call.
        let layouts: Vec<Layout<'b>> = layout.children().collect();
        let has_middle = self.middle.is_some();

        // Split the tree children into disjoint mutable slots.
        let (lead_tree, rest) = tree.children.split_at_mut(1);
        if let Some(ll) = layouts.first() {
            if let Some(ov) = self.lead.as_widget_mut().overlay(
                &mut lead_tree[0],
                *ll,
                renderer,
                viewport,
                translation,
            ) {
                return Some(ov);
            }
        }

        if has_middle {
            let (mid_tree, trail_tree) = rest.split_at_mut(1);
            if let (Some(m), Some(ml)) = (self.middle.as_mut(), layouts.get(1)) {
                if let Some(ov) = m.as_widget_mut().overlay(
                    &mut mid_tree[0],
                    *ml,
                    renderer,
                    viewport,
                    translation,
                ) {
                    return Some(ov);
                }
            }
            if let Some(tl) = layouts.get(2) {
                return self.trail.as_widget_mut().overlay(
                    &mut trail_tree[0],
                    *tl,
                    renderer,
                    viewport,
                    translation,
                );
            }
        } else if let Some(tl) = layouts.get(1) {
            return self.trail.as_widget_mut().overlay(
                &mut rest[0],
                *tl,
                renderer,
                viewport,
                translation,
            );
        }
        None
    }
}

impl<'a> From<WrapBar<'a>> for Element<'a, Message> {
    fn from(w: WrapBar<'a>) -> Self {
        Element::new(w)
    }
}

/// Flex-wrap row: lays its items left-to-right and wraps onto a new row when
/// the next item would exceed the available width. Each row is `row_h` tall and
/// items are vertically centred. Used for the status-bar pills so they spread
/// across multiple rows when the width can't hold them on one line.
pub struct WrapFlow<'a> {
    items: Vec<Element<'a, Message>>,
    spacing_x: f32,
    /// Smallest inter-item gap when the row overflows. `INFINITY` (the default)
    /// means "never compress" — strictly opt-in per flow.
    min_spacing_x: f32,
    spacing_y: f32,
    row_h: f32,
}

impl<'a> WrapFlow<'a> {
    pub fn new(items: Vec<Element<'a, Message>>) -> Self {
        Self {
            items,
            spacing_x: 2.0,
            min_spacing_x: f32::INFINITY,
            spacing_y: 0.0,
            row_h: 28.0,
        }
    }

    pub fn spacing_x(mut self, s: f32) -> Self {
        self.spacing_x = s;
        self
    }

    /// Allow the inter-item gap to shrink to `s` (from `spacing_x`) when the
    /// items don't fit, so they get a bit closer before wrapping to a new row.
    pub fn min_spacing_x(mut self, s: f32) -> Self {
        self.min_spacing_x = s;
        self
    }

    pub fn row_h(mut self, h: f32) -> Self {
        self.row_h = h;
        self
    }
}

impl<'a> Widget<Message, Theme, Renderer> for WrapFlow<'a> {
    fn children(&self) -> Vec<widget::Tree> {
        self.items.iter().map(widget::Tree::new).collect()
    }

    fn diff(&self, tree: &mut widget::Tree) {
        let refs: Vec<_> = self.items.iter().map(|e| e.as_widget()).collect();
        tree.diff_children(&refs);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let max_w = limits.max().width;
        let natural =
            layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, f32::INFINITY));

        // Measure each item exactly once and total their widths.
        let mut measured: Vec<layout::Node> = Vec::with_capacity(self.items.len());
        let mut sum_w = 0.0f32;
        for (item, state) in self.items.iter_mut().zip(tree.children.iter_mut()) {
            let node = item.as_widget_mut().layout(state, renderer, &natural);
            sum_w += node.size().width;
            measured.push(node);
        }

        // If this flow opted into compression and the items don't fit one row at
        // the normal gap, shrink the gap (down to min_spacing_x) so they get a
        // bit closer before wrapping. Only fires when max_w is a real bound
        // (i.e. the flow is already width-constrained, e.g. overflowing tabs).
        let n = measured.len();
        let mut eff_gap = self.spacing_x;
        if self.min_spacing_x < self.spacing_x && max_w.is_finite() && n > 1 {
            let line_w = sum_w + (n - 1) as f32 * self.spacing_x;
            if line_w > max_w {
                eff_gap =
                    ((max_w - sum_w) / (n - 1) as f32).clamp(self.min_spacing_x, self.spacing_x);
            }
        }

        let mut nodes: Vec<layout::Node> = Vec::with_capacity(n);
        let mut x = 0.0f32;
        let mut y = 0.0f32;
        let mut used_w = 0.0f32;

        for node in measured {
            let sz = node.size();
            if x > 0.0 && x + sz.width > max_w {
                x = 0.0;
                y += self.row_h + self.spacing_y;
            }
            let cy = y + ((self.row_h - sz.height) / 2.0).max(0.0);
            nodes.push(node.move_to(Point::new(x, cy)));
            x += sz.width + eff_gap;
            used_w = used_w.max(x - eff_gap);
        }

        let total_h = if self.items.is_empty() {
            self.row_h
        } else {
            y + self.row_h
        };
        layout::Node::with_children(Size::new(used_w.max(0.0), total_h), nodes)
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for ((item, state), child_layout) in self
            .items
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(layout.children())
        {
            item.as_widget_mut().update(
                state,
                event,
                child_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let mut interaction = mouse::Interaction::default();
        for ((item, state), child_layout) in self
            .items
            .iter()
            .zip(tree.children.iter())
            .zip(layout.children())
        {
            let i = item.as_widget().mouse_interaction(
                state,
                child_layout,
                cursor,
                viewport,
                renderer,
            );
            if i != mouse::Interaction::default() {
                interaction = i;
            }
        }
        interaction
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        for ((item, state), child_layout) in self
            .items
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(layout.children())
        {
            item.as_widget_mut()
                .operate(state, child_layout, renderer, operation);
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for ((item, state), child_layout) in self
            .items
            .iter()
            .zip(tree.children.iter())
            .zip(layout.children())
        {
            item.as_widget()
                .draw(state, renderer, theme, style, child_layout, cursor, viewport);
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        overlay::from_children(
            &mut self.items,
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a> From<WrapFlow<'a>> for Element<'a, Message> {
    fn from(w: WrapFlow<'a>) -> Self {
        Element::new(w)
    }
}

/// Picks the widest variant that fits the available width and shows only that
/// one — used by the ribbon tool area to swap a full-size panel row for a
/// compact (icon-only) one when the window is too narrow. Variants must be
/// ordered widest-first; the last is shown when none fit.
pub struct DensitySwap<'a> {
    variants: Vec<Element<'a, Message>>,
    chosen: Cell<usize>,
    height_out: Option<Arc<AtomicU32>>,
    /// Receives the FIRST (widest) variant's natural width in bits of an `f32`,
    /// measured every layout regardless of which variant is shown — so a caller
    /// can place a neighbouring widget relative to the full-size content even
    /// while a narrower variant is on screen.
    width0_out: Option<Arc<AtomicU32>>,
}

impl<'a> DensitySwap<'a> {
    pub fn new(variants: Vec<Element<'a, Message>>) -> Self {
        Self {
            variants,
            chosen: Cell::new(0),
            height_out: None,
            width0_out: None,
        }
    }

    /// Report the chosen variant's height (bits of an `f32`) so callers can
    /// anchor overlays below a possibly-taller (wrapped) tool area.
    pub fn report_height(mut self, out: Arc<AtomicU32>) -> Self {
        self.height_out = Some(out);
        self
    }

    /// Report the first variant's natural (unconstrained) width — see `width0_out`.
    pub fn report_width0(mut self, out: Arc<AtomicU32>) -> Self {
        self.width0_out = Some(out);
        self
    }
}

impl<'a> Widget<Message, Theme, Renderer> for DensitySwap<'a> {
    fn children(&self) -> Vec<widget::Tree> {
        self.variants.iter().map(widget::Tree::new).collect()
    }

    fn diff(&self, tree: &mut widget::Tree) {
        let refs: Vec<_> = self.variants.iter().map(|e| e.as_widget()).collect();
        tree.diff_children(&refs);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let max_w = limits.max().width;
        let natural =
            layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, f32::INFINITY));

        // Widest-first: keep the first variant whose natural width fits; else the
        // last (which is expected to wrap to fit any width).
        let mut pick = self.variants.len().saturating_sub(1);
        for (i, v) in self.variants.iter_mut().enumerate() {
            let n = v.as_widget_mut().layout(&mut tree.children[i], renderer, &natural);
            if i == 0 {
                if let Some(out) = &self.width0_out {
                    out.store(n.size().width.to_bits(), Ordering::Relaxed);
                }
            }
            if n.size().width <= max_w {
                pick = i;
                break;
            }
        }
        self.chosen.set(pick);

        let node =
            self.variants[pick]
                .as_widget_mut()
                .layout(&mut tree.children[pick], renderer, limits);
        let sz = node.size();
        if let Some(out) = &self.height_out {
            out.store(sz.height.to_bits(), Ordering::Relaxed);
        }
        layout::Node::with_children(sz, vec![node])
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let i = self.chosen.get();
        if let Some(child_layout) = layout.children().next() {
            self.variants[i].as_widget_mut().update(
                &mut tree.children[i],
                event,
                child_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let i = self.chosen.get();
        layout
            .children()
            .next()
            .map(|child_layout| {
                self.variants[i].as_widget().mouse_interaction(
                    &tree.children[i],
                    child_layout,
                    cursor,
                    viewport,
                    renderer,
                )
            })
            .unwrap_or_default()
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let i = self.chosen.get();
        if let Some(child_layout) = layout.children().next() {
            self.variants[i]
                .as_widget_mut()
                .operate(&mut tree.children[i], child_layout, renderer, operation);
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let i = self.chosen.get();
        if let Some(child_layout) = layout.children().next() {
            self.variants[i].as_widget().draw(
                &tree.children[i],
                renderer,
                theme,
                style,
                child_layout,
                cursor,
                viewport,
            );
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let i = self.chosen.get();
        let child_layout = layout.children().next()?;
        self.variants[i].as_widget_mut().overlay(
            &mut tree.children[i],
            child_layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a> From<DensitySwap<'a>> for Element<'a, Message> {
    fn from(w: DensitySwap<'a>) -> Self {
        Element::new(w)
    }
}

/// A transparent wrapper that records its child's screen bounds under `id` on
/// every draw, so an open dropdown can anchor its overlay just below the widget.
pub struct PosReport<'a> {
    id: std::borrow::Cow<'static, str>,
    child: Element<'a, Message>,
}

impl<'a> PosReport<'a> {
    pub fn new(id: &'static str, child: impl Into<Element<'a, Message>>) -> Self {
        Self {
            id: std::borrow::Cow::Borrowed(id),
            child: child.into(),
        }
    }

    /// Report under a runtime-built id (e.g. one per layout tab).
    pub fn owned(id: String, child: impl Into<Element<'a, Message>>) -> Self {
        Self {
            id: std::borrow::Cow::Owned(id),
            child: child.into(),
        }
    }
}

impl<'a> Widget<Message, Theme, Renderer> for PosReport<'a> {
    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.child)]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(&[self.child.as_widget()]);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.child.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        DD_BOUNDS.with(|m| {
            m.borrow_mut().insert(self.id.to_string(), layout.bounds());
        });
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a> From<PosReport<'a>> for Element<'a, Message> {
    fn from(w: PosReport<'a>) -> Self {
        Element::new(w)
    }
}
