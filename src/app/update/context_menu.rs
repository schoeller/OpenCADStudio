//! Viewport right-click context menu: gathering the menu's inputs from app
//! state, running a picked row, and keyboard navigation while it is open.
//!
//! The rows themselves are built by `ui::popup::context_menu` (a pure model);
//! this module is the glue between that model and the app's messages.

/// Quad display-option messages, shared by the Options window, the status
/// bar's QUAD popup and the `SETVAR` reflection of the quad sysvars. Kept
/// here (next to the other quad glue) and re-exported via
/// `crate::QuadOptionMsg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuadOptionMsg {
    /// The quad cursor menu exists at all (QUAD).
    Quad(bool),
    /// QUADDISPLAY: hovering an entity offers the quad.
    OnHover(bool),
    /// QUADDISPLAY: a selection set offers the quad.
    OnSelect(bool),
    /// QUADDISPLAY: a targetless right-click opens the No Selection quad.
    OnRightClick(bool),
    /// QUADDISPLAY: hovering is suppressed while something is selected.
    SuppressHoverWhenSelection(bool),
    /// QUADWIDTH: columns of the expanded quad (clamped in settings).
    Width(u8),
    /// QUADCOMMANDLAUNCH: right-click over an entity launches its default
    /// command directly.
    CommandLaunch(bool),
    /// Milliseconds of rest before the hover quad opens (clamped in
    /// settings).
    HoverDelay(u64),
    /// The hover quad's ACIS subentity resolution gate over a solid
    /// (docs/quad-menu.md §3.5.1): 0 = never, 1 = Shift+hover (default),
    /// 2 = plain hover (clamped in settings; no sysvar reflects it).
    AcisFaces(u8),
}

use crate::app::{ArrowKey, ContextMenuNav, Message, OpenCADStudio};
use glam::DVec3;
use crate::command::StepInput;
use crate::scene::pick::grip::GripEditMode;
use crate::ui::popup::context_menu::{
    build_context_menu, ContextMenu, GripMenuCmd, GripMenuContext, MenuAction, MenuContext,
    SubmenuId,
};
use crate::scene::pick::subentity::SubTarget;
use crate::ui::popup::quad_menu::{
    quad_panels_for_target, quad_subentity_gate, QuadMode, QuadMsg, QuadTarget, QuadUi,
};
use codec::Handle;
use iced::{Point, Task};

impl OpenCADStudio {
    /// True while the viewport context menu is open on the active tab.
    pub(in crate::app) fn context_menu_open(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.scene.selection.borrow().menu.open_at.is_some())
    }

    /// Everything the menu builder needs, read from the active tab. Called by
    /// the view (to render) and by the keyboard handler (to resolve picks), so
    /// both always see the same rows.
    pub(in crate::app) fn context_menu_context(&self) -> MenuContext {
        let tab = &self.tabs[self.active_tab];
        if let Some(grip) = tab.active_grip.as_ref() {
            return MenuContext::Grip(GripMenuContext {
                mode: grip.mode,
                copy_on: tab.grip_copy,
                moved: grip.last_world != grip.origin_world,
            });
        }
        if let Some(cmd) = tab.active_cmd.as_ref() {
            // Same guard as typed MTP/M2P and SnapOverrideMtp: the step must
            // take a point pick (possibly alongside keywords), not an entity.
            let has_point_step = (!cmd.input_kind().wants_text()
                || cmd.point_step_accepts_keywords())
                && !cmd.needs_entity_pick();
            return MenuContext::Command {
                options: cmd.options(),
                has_point_step,
                recent_inputs: self.command_line.recent_inputs.clone(),
            };
        }
        MenuContext::Idle {
            has_selection: !tab.scene.selected.is_empty(),
            single_hatch: tab.scene.selected.len() == 1
                && tab.scene.selected.iter().all(|handle| {
                    matches!(
                        tab.scene.document.get_entity(*handle),
                        Some(codec::EntityType::Hatch(_))
                    )
                }),
            single_mleader: tab.scene.selected.len() == 1
                && tab.scene.selected.iter().all(|handle| {
                    matches!(
                        tab.scene.document.get_entity(*handle),
                        Some(codec::EntityType::MultiLeader(_))
                    )
                }),
            selected_constraint: tab.scene.selected_constraint,
            isolation_active: tab.scene.has_hidden_objects(),
            recent_cmds: self
                .command_line
                .recent_commands
                .iter()
                .rev()
                .take(crate::ui::popup::context_menu::RECENT_LIMIT)
                .cloned()
                .collect(),
            clipboard_nonempty: !self.clipboard.is_empty(),
            undo_label: tab.history.undo_stack.last().map(|s| s.display_label().into_owned()),
            redo_label: tab.history.redo_stack.last().map(|s| s.display_label().into_owned()),
            props_open: self.show_properties,
        }
    }

    /// The menu as currently shown (rows + expanded submenu).
    pub(in crate::app) fn current_context_menu(&self) -> ContextMenu {
        let open = self.tabs[self.active_tab]
            .scene
            .selection
            .borrow()
            .menu
            .ui
            .open_submenu;
        build_context_menu(&self.context_menu_context(), open)
    }

    /// Close the menu and give the keyboard back to the command line.
    pub(in crate::app) fn close_context_menu(&mut self) -> Task<Message> {
        let i = self.active_tab;
        let mut sel = self.tabs[i].scene.selection.borrow_mut();
        if sel.menu.open_at.is_none() {
            return Task::none();
        }
        sel.menu.close();
        drop(sel);
        self.focus_cmd_input()
    }

    pub(in crate::app) fn on_context_menu_submenu_toggle(
        &mut self,
        id: SubmenuId,
    ) -> Task<Message> {
        let i = self.active_tab;
        let mut sel = self.tabs[i].scene.selection.borrow_mut();
        let ui = &mut sel.menu.ui;
        ui.open_submenu = if ui.open_submenu == Some(id) {
            None
        } else {
            Some(id)
        };
        drop(sel);
        // Keep the keyboard highlight on the header that was toggled.
        let menu = self.current_context_menu();
        let header = menu.selectable().iter().position(|s| s.header == Some(id));
        let mut sel = self.tabs[i].scene.selection.borrow_mut();
        if sel.menu.ui.highlighted.is_some() {
            sel.menu.ui.highlighted = header;
        }
        Task::none()
    }

    /// Run a picked row. The menu closes first so the action sees the same
    /// state a typed keyword / shortcut would (no menu open), then the action
    /// is routed through the existing message for that behaviour. Prompts
    /// focus the command line; a completed Select Similar returns to the canvas.
    pub(in crate::app) fn on_context_menu_pick(&mut self, action: MenuAction) -> Task<Message> {
        if let MenuAction::ToggleSubmenu(id) = action {
            return self.on_context_menu_submenu_toggle(id);
        }
        let i = self.active_tab;
        {
            let mut sel = self.tabs[i].scene.selection.borrow_mut();
            sel.menu.close();
            // A pick counts as "another interaction" for the Enter-first
            // right-click cycle: the next right-click acts as Enter again.
            sel.input.right_click_entered = false;
        }
        // The selection rows finish a canvas selection, not a command prompt.
        // An empty focused command input captures the following Ctrl+C.
        let focus = if matches!(
            action,
            MenuAction::SelectSimilar | MenuAction::InvertSelection | MenuAction::DeselectAll
        ) {
            self.unfocus_widgets()
        } else {
            self.focus_cmd_input()
        };
        let task = match action {
            MenuAction::Enter if self.tabs[i].active_grip.is_some() => {
                self.commit_active_grip_edit()
            }
            MenuAction::Enter => self.update(Message::CommandFinalize),
            MenuAction::Cancel => self.update(Message::CommandEscape),
            MenuAction::Option(kw) => self.update(Message::CommandOptionPick(kw)),
            MenuAction::FeedInput(token) => {
                if self.tabs[i].active_cmd.is_some() {
                    self.feed_active_cmd(&token)
                } else {
                    self.run_command_line(&token)
                }
            }
            MenuAction::Command(cmd) => self.update(Message::Command(cmd)),
            MenuAction::Mtp => self.update(Message::SnapOverrideMtp),
            MenuAction::From => self.update(Message::SnapOverrideFrom),
            MenuAction::SnapOverride(t) => self.update(Message::SnapOverridePick(t)),
            MenuAction::SnapOverrideNone => self.update(Message::SnapOverrideNone),
            MenuAction::DeleteSelected => self.update(Message::DeleteSelected),
            MenuAction::SelectSimilar => self.update(Message::SelectSimilar),
            MenuAction::InvertSelection => self.update(Message::InvertSelection),
            MenuAction::DeselectAll => {
                self.tabs[i].scene.deselect_all();
                self.refresh_properties();
                Task::none()
            }
            MenuAction::QuickSelect => self.update(Message::QSelectOpen),
            MenuAction::Properties => self.update(Message::ToggleProperties),
            MenuAction::Options => self.update(Message::OptionsOpen),
            MenuAction::Undo => self.update(Message::Undo),
            MenuAction::Redo => self.update(Message::Redo),
            MenuAction::DrawOrderPickRef(above) => self.update(Message::DrawOrderPickRef(above)),
            MenuAction::ConstraintDelete(id) => self.update(Message::PropConstraintDelete(id)),
            MenuAction::Grip(cmd) => self.on_grip_context_pick(cmd),
            MenuAction::ToggleSubmenu(_) => Task::none(),
        };
        Task::batch(vec![focus, task])
    }

    /// Keyboard navigation while the menu is open.
    pub(in crate::app) fn on_context_menu_navigate(
        &mut self,
        nav: ContextMenuNav,
    ) -> Task<Message> {
        if !self.context_menu_open() {
            return Task::none();
        }
        let i = self.active_tab;
        let menu = self.current_context_menu();
        let rows = menu.selectable();
        if rows.is_empty() {
            return Task::none();
        }
        let current = self.tabs[i].scene.selection.borrow().menu.ui.highlighted;
        let next_enabled = |from: usize, step: isize| -> usize {
            let n = rows.len() as isize;
            let mut idx = from as isize;
            for _ in 0..n {
                idx = (idx + step).rem_euclid(n);
                if rows[idx as usize].enabled {
                    return idx as usize;
                }
            }
            from
        };
        match nav {
            ContextMenuNav::Down => {
                let idx = match current {
                    // First press lands on the default row itself.
                    None => menu.default_index(),
                    Some(cur) => next_enabled(cur, 1),
                };
                self.tabs[i]
                    .scene
                    .selection
                    .borrow_mut()
                    .menu
                    .ui
                    .highlighted = Some(idx);
                Task::none()
            }
            ContextMenuNav::Up => {
                let idx = match current {
                    None => menu.default_index(),
                    Some(cur) => next_enabled(cur, -1),
                };
                self.tabs[i]
                    .scene
                    .selection
                    .borrow_mut()
                    .menu
                    .ui
                    .highlighted = Some(idx);
                Task::none()
            }
            ContextMenuNav::Enter => {
                let idx = current.unwrap_or_else(|| menu.default_index());
                let row = &rows[idx];
                if !row.enabled {
                    return Task::none();
                }
                self.on_context_menu_pick(row.action.clone())
            }
            ContextMenuNav::Mnemonic(ch) => match menu.find_mnemonic(ch, current) {
                Some(idx) => {
                    // A letter shared by several rows (PLINE's CEnter / CLose)
                    // cycles the highlight; a unique one picks at once.
                    let unique = menu.find_mnemonic(ch, Some(idx)) == Some(idx);
                    if unique {
                        self.on_context_menu_pick(rows[idx].action.clone())
                    } else {
                        self.tabs[i]
                            .scene
                            .selection
                            .borrow_mut()
                            .menu
                            .ui
                            .highlighted = Some(idx);
                        Task::none()
                    }
                }
                None => Task::none(),
            },
        }
    }

    /// Route keyboard-derived messages to the open menu. Returns `Some(task)`
    /// when the message was consumed; `None` lets it fall through to the
    /// normal handlers (after closing the menu when the key is not a menu key,
    /// so typing a command name or a coordinate just works — the menu gets
    /// out of the way exactly as it does in commercial solutions).
    pub(in crate::app) fn intercept_context_menu_key(
        &mut self,
        msg: &Message,
    ) -> Option<Task<Message>> {
        match msg {
            Message::ArrowKeyPressed { direction, .. }
            | Message::CommandLineArrowProbe { direction, .. } => match direction {
                ArrowKey::Up => Some(self.on_context_menu_navigate(ContextMenuNav::Up)),
                ArrowKey::Down => Some(self.on_context_menu_navigate(ContextMenuNav::Down)),
                ArrowKey::Left | ArrowKey::Right => {
                    // On a submenu header, Right expands and Left collapses.
                    let i = self.active_tab;
                    let ui = self.tabs[i].scene.selection.borrow().menu.ui.clone();
                    let menu = self.current_context_menu();
                    let rows = menu.selectable();
                    let header = ui
                        .highlighted
                        .and_then(|c| rows.get(c))
                        .and_then(|r| r.header);
                    let want_open = matches!(direction, ArrowKey::Right);
                    match header {
                        Some(id) if (ui.open_submenu == Some(id)) != want_open => {
                            Some(self.on_context_menu_submenu_toggle(id))
                        }
                        _ => Some(Task::none()),
                    }
                }
            },
            Message::DynTabNext => Some(self.on_context_menu_navigate(ContextMenuNav::Down)),
            Message::DynTabPrev => Some(self.on_context_menu_navigate(ContextMenuNav::Up)),
            Message::CommandFinalize | Message::CommandSpace => {
                if self.command_line.input.trim().is_empty() {
                    Some(self.on_context_menu_navigate(ContextMenuNav::Enter))
                } else {
                    // Typed text pending: Enter runs it, the menu just closes.
                    let close = self.close_context_menu();
                    let run = self.update(Message::CommandFinalize);
                    Some(Task::batch(vec![close, run]))
                }
            }
            Message::CommandSubmit => {
                if self.command_line.input.trim().is_empty() {
                    Some(self.on_context_menu_navigate(ContextMenuNav::Enter))
                } else {
                    let _ = self.close_context_menu();
                    None
                }
            }
            Message::CommandAppendChar(s) => self.context_menu_typed(s.chars().next()),
            Message::CommandInput(buf) => {
                // The focused command-line field delivers the whole buffer;
                // only a single appended character is a candidate mnemonic.
                let appended = buf
                    .strip_prefix(self.command_line.input.as_str())
                    .filter(|rest| rest.chars().count() == 1)
                    .and_then(|rest| rest.chars().next());
                match appended {
                    Some(ch) => self.context_menu_typed(Some(ch)),
                    None => {
                        let _ = self.close_context_menu();
                        None
                    }
                }
            }
            _ => None,
        }
    }

    /// Keep Escape, Enter, Space and the arrows out of the command line and the
    /// viewport while a property list is open: they would otherwise dismiss the
    /// selection the list was opened for. Escape closes the list.
    pub(in crate::app) fn intercept_property_list_key(
        &mut self,
        msg: &Message,
    ) -> Option<Task<Message>> {
        let base = match msg {
            Message::ShortcutPressed(key) => key.rsplit('+').next(),
            _ => None,
        };
        let escape = matches!(msg, Message::CommandEscape) || matches!(base, Some("ESCAPE"));
        let consumed = escape
            || matches!(
                msg,
                Message::ArrowKeyPressed { .. }
                    | Message::CommandLineArrowProbe { .. }
                    | Message::CommandLineSpaceProbe
                    | Message::CommandFinalize
                    | Message::CommandSubmit
                    | Message::CommandSpace
            )
            || matches!(base, Some("ENTER" | "RETURN" | "SPACE"));
        if !consumed {
            self.block_picker_dismissed = false;
            return None;
        }
        let panel = &self.tabs[self.active_tab].properties;
        let any_open = panel.open_block_picker.is_some() || panel.open_choice.is_some();
        if !any_open {
            // The overlay reported its own Escape a moment earlier; the global
            // shortcut repeats the same keypress with the list already closed.
            // Dropping that repeat keeps the selection; any other key ends the
            // window without being swallowed.
            if self.block_picker_dismissed && escape {
                self.block_picker_dismissed = false;
                return Some(Task::none());
            }
            self.block_picker_dismissed = false;
            return None;
        }
        if escape {
            let panel = &mut self.tabs[self.active_tab].properties;
            panel.open_block_picker = None;
            panel.open_choice = None;
        }
        Some(Task::none())
    }

    /// A printable key while the menu is open: a mnemonic picks its row;
    /// anything else closes the menu and is typed into the command line.
    fn context_menu_typed(&mut self, ch: Option<char>) -> Option<Task<Message>> {
        let ch = ch?;
        let menu = self.current_context_menu();
        if ch.is_ascii_alphanumeric() && menu.find_mnemonic(ch, None).is_some() {
            return Some(self.on_context_menu_navigate(ContextMenuNav::Mnemonic(ch)));
        }
        let _ = self.close_context_menu();
        None
    }

    /// Grip-mode shortcut menu (the grip menu of commercial solutions): the grip stays hot
    /// for Stretch / Base Point / Copy / Undo; Move / Rotate / Scale / Mirror
    /// hand the selection to the matching command with the grip as its base
    /// point; Exit drops the edit.
    pub(in crate::app) fn on_grip_context_pick(&mut self, cmd: GripMenuCmd) -> Task<Message> {
        let i = self.active_tab;
        let Some(grip) = self.tabs[i].active_grip.clone() else {
            return Task::none();
        };
        match cmd {
            GripMenuCmd::Exit => {
                self.cancel_active_grip_edit();
                self.tabs[i].grip_copy = false;
                self.tabs[i].grip_base_pending = false;
                Task::none()
            }
            GripMenuCmd::Stretch => {
                if let Some(active) = self.tabs[i].active_grip.as_mut() {
                    active.mode = GripEditMode::Stretch;
                    active.axis = None;
                }
                Task::none()
            }
            GripMenuCmd::Move | GripMenuCmd::Rotate | GripMenuCmd::Scale | GripMenuCmd::Mirror => {
                let base = grip.origin_world;
                let handles: Vec<Handle> = grip.targets.iter().map(|t| t.handle).collect();
                self.cancel_active_grip_edit();
                self.tabs[i].grip_copy = false;
                self.tabs[i].grip_base_pending = false;
                // The command edits the current selection; make sure the
                // gripped objects are in it.
                if self.tabs[i].scene.selected.is_empty() {
                    self.tabs[i].scene.select_entities(&handles);
                }
                let name = match cmd {
                    GripMenuCmd::Move => "MOVE",
                    GripMenuCmd::Rotate => "ROTATE",
                    GripMenuCmd::Scale => "SCALE",
                    _ => "MIRROR",
                };
                let start = self.dispatch_command(name);
                if self.tabs[i].active_cmd.is_none() {
                    return start;
                }
                // The grip is the base point, so the command opens directly
                // at its second prompt.
                self.last_point = Some(base);
                let fed = self.feed_command(StepInput::Point(base));
                Task::batch(vec![start, fed])
            }
            GripMenuCmd::BasePoint => {
                self.tabs[i].grip_base_pending = true;
                Task::none()
            }
            GripMenuCmd::CopyToggle => {
                self.tabs[i].grip_copy = !self.tabs[i].grip_copy;
                Task::none()
            }
            GripMenuCmd::Undo => {
                // Put the shapes back where the gesture started and keep the
                // grip hot there (the Copy toggle survives).
                let copy = self.tabs[i].grip_copy;
                self.cancel_active_grip_edit();
                self.tabs[i].grip_copy = copy;
                let mut again = grip;
                let delta = again.last_world - again.origin_world;
                again.last_world = again.origin_world;
                for target in &mut again.targets {
                    target.last_world -= delta;
                }
                self.tabs[i].active_grip = Some(again);
                Task::none()
            }
        }
    }
}

// ── Quad cursor menu ─────────────────────────────────────────────────────
//
// State, routers and dispatch glue for the quad cursor menu
// (docs/quad-menu.md). The window itself is the parallel overlay's; this
// side owns what it consumes: the open/close lifecycle on the selection
// boundary, the panel model built once at open time, the pick dispatch and
// the hover-dwell bookkeeping.

/// How far the cursor may wander (px) from where a hover quad opened before
/// the quad closes again (docs/quad-menu.md §3.4).
pub(in crate::app) const QUAD_DRIFT_PX: f32 = 8.0;

/// Commands whose line starts with this are plugin quad actions; the pick
/// router hands them to `OpenCADStudio::quad_plugin_action` (the pick path's
/// own host dispatch) instead of the generic dispatcher — the scheme is the
/// pick surface's own and impossible by typed/automation input. The literal
/// lives in `crate::plugin` (the scheme's home); this alias keeps the
/// router's name.
pub(crate) const QUAD_PLUGIN_PREFIX: &str = crate::plugin::QUAD_PLUGIN_PREFIX;

impl OpenCADStudio {
    /// True while the quad cursor menu is open on the active tab.
    pub(in crate::app) fn quad_open(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.scene.selection.borrow().menu.quad.is_some())
    }

    /// True while a qualifying hover dwell is being counted but the quad has
    /// not opened yet — the settled-rollover tick keeps re-arming itself so
    /// the delayed open can cross even without further pointer moves.
    pub(in crate::app) fn quad_hover_counting(&self) -> bool {
        self.quad_hover_since.is_some() && !self.quad_open()
    }

    /// Classify what the cursor is on (docs/quad-menu.md §3.2): the hovered
    /// entity first, else the selection set (its type keys in pick order),
    /// else the void.
    pub(in crate::app) fn quad_target(&self) -> QuadTarget {
        let tab = &self.tabs[self.active_tab];
        if let Some(handle) = tab.scene.hover_highlight {
            if let Some(entity) = tab.scene.document.get_entity(handle) {
                return QuadTarget::Hovered {
                    handle,
                    type_key: crate::entities::traits::entity_type_name(entity).to_string(),
                    // The plain classification carries no subentity; only
                    // the hover quad's own paths resolve one
                    // ([`Self::quad_hover_classified`], docs/quad-menu.md
                    // §3.5.1).
                    sub: None,
                };
            }
        }
        self.quad_selection_target()
    }

    /// The active tab's selection set as a quad target (`QuadTarget::None`
    /// on an empty selection) — the variant a Selection quad presents even
    /// while its cursor still hovers one of the selected objects.
    fn quad_selection_target(&self) -> QuadTarget {
        let tab = &self.tabs[self.active_tab];
        if tab.scene.selected.is_empty() {
            return QuadTarget::None;
        }
        let handles = tab.scene.selected_handles_in_order();
        let type_keys = handles
            .iter()
            .filter_map(|handle| {
                tab.scene
                    .document
                    .get_entity(*handle)
                    .map(|entity| crate::entities::traits::entity_type_name(entity).to_string())
            })
            .collect();
        QuadTarget::Selection { handles, type_keys }
    }

    /// Resolve the target a quad of `mode` is about: a Hover quad is about
    /// the hovered entity, a Selection quad about the selection set (even
    /// while the cursor still hovers one of its objects), and the No
    /// Selection quad is always the void.
    fn quad_target_for(&self, mode: QuadMode) -> QuadTarget {
        match mode {
            QuadMode::Hover => self.quad_target(),
            QuadMode::Selection => self.quad_selection_target(),
            QuadMode::NoSelection => QuadTarget::None,
        }
    }

    /// The visible plugins' registered quad panels (docs/quad-menu.md §4,
    /// API v15): registration order, the disabled and dead dropped. The web
    /// build has no plugin surface.
    fn quad_plugin_panels(&self) -> Vec<crate::ui::popup::quad_menu::QuadPanel> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = self;
            return Vec::new();
        }
        #[cfg(not(target_arch = "wasm32"))]
        crate::app::plugin_host::plugin_quad_panels(&self.disabled_plugins)
    }

    /// The hovered entity's subentity candidates for the quad
    /// (docs/quad-menu.md §3.5.1), nearest first — gated by
    /// `quad_acis_faces` × Shift ([`quad_subentity_gate`]): 0 = never
    /// resolve, 1 = only while Shift is held, 2 = always. The resolution
    /// radius is the pick aperture in world units, so the quad's richer
    /// answer shares the pick's forgiveness; with no model-space view scale
    /// (paper space, no settled frame) nothing resolves and the quad keeps
    /// its solid level.
    fn quad_subentity_candidates(&self, i: usize, handle: Handle, world: DVec3) -> Vec<SubTarget> {
        if !quad_subentity_gate(self.quad_acis_faces, self.shift_down) {
            return Vec::new();
        }
        let scene = &self.tabs[i].scene;
        let Some(world_per_px) = scene.world_per_pixel() else {
            return Vec::new();
        };
        let aperture = crate::ui::overlay::pick_box_aperture_px(self.pick_box) as f64;
        crate::scene::pick::subentity::subentity_candidates(
            scene,
            handle,
            world,
            aperture * world_per_px as f64,
        )
    }

    /// The Hover quad's classification *with* subentity resolution
    /// (docs/quad-menu.md §3.5.1): over a face-bearing solid the nearest
    /// candidate becomes [`QuadTarget::Hovered`]'s `sub` (the primary panel
    /// switches to the kind's functions) and every candidate returns so the
    /// keyboard Tab can cycle them. Selection / NoSelection /
    /// hover-highlight-less targets never resolve subentities.
    fn quad_hover_classified(&self, i: usize, world: DVec3) -> (QuadTarget, Vec<SubTarget>) {
        let target = self.quad_target();
        let QuadTarget::Hovered {
            handle,
            type_key,
            sub: None,
        } = &target
        else {
            return (target, Vec::new());
        };
        let (handle, type_key) = (*handle, type_key.clone());
        let candidates = self.quad_subentity_candidates(i, handle, world);
        let sub = candidates.first().copied();
        (
            QuadTarget::Hovered {
                handle,
                type_key,
                sub,
            },
            candidates,
        )
    }

    /// Drive the quad's subentity highlight (docs/quad-menu.md §3.5.1):
    /// the Stage-1 `subentity_highlight_wires` geometry through the same
    /// preview-wire overlay the PRESSPULL boundary hover feeds, tagged on
    /// that wire's naming scheme. `None` (solid level, a body that fell
    /// away) clears. The quad lives outside any command, so the preview
    /// wires are *its* overlay surface while open — and
    /// [`Self::close_quad`] hands them back.
    fn quad_refresh_subentity_highlight(&mut self, i: usize, handle: Handle, sub: Option<SubTarget>) {
        let wires = match sub {
            Some(sub) => crate::scene::pick::subentity::subentity_highlight_wires(
                &self.tabs[i].scene,
                handle,
                sub,
            ),
            None => Vec::new(),
        };
        let mut models: Vec<crate::scene::WireModel> = Vec::new();
        for points in wires {
            if points.len() < 2 {
                continue;
            }
            let mut wire = crate::scene::WireModel::solid_f64(
                "QUAD-SUBENTITY-HOVER".to_owned(),
                points.into_iter().map(|p| p.to_array()).collect::<Vec<_>>(),
                crate::scene::WireModel::HOVER,
                false,
            );
            wire.line_weight_px = 2.0;
            models.push(wire);
        }
        if models.is_empty() {
            self.tabs[i].scene.clear_preview_wire();
        } else {
            self.tabs[i].scene.set_preview_wires(models);
        }
    }

    /// Rebuild an open Hover quad in place for `target`
    /// (docs/quad-menu.md §3.5.1): panels, open-target classification,
    /// staged candidates + cycle index and the subentity highlight —
    /// everything but the anchor recomputed. `expanded`/`active_tab` are the
    /// previous window state, preserved by the callers (a solid-level fold
    /// collapses first). A target with nothing to offer closes the quad —
    /// same offer law as [`Self::open_quad`].
    fn quad_rebuild_hover_panels(
        &mut self,
        i: usize,
        target: QuadTarget,
        candidates: Vec<SubTarget>,
        sub_index: usize,
        expanded: bool,
        active_tab: usize,
    ) {
        let plugin_panels = self.quad_plugin_panels();
        let shift = self.shift_down;
        let panels = quad_panels_for_target(
            &target,
            &self.quad_mru,
            self.quad_width as usize,
            &plugin_panels,
            self.quad_acis_faces,
            shift,
        );
        if !panels.iter().any(|panel| !panel.items.is_empty()) {
            self.close_quad();
            return;
        }
        let mut sel = self.tabs[i].scene.selection.borrow_mut();
        if let Some(quad) = sel.menu.quad.as_mut() {
            quad.sub_candidates = candidates;
            quad.sub_index = sub_index;
            // The kept tab index clamps to the NEW panel count: kind
            // switches change how many group tabs exist.
            quad.active_tab = active_tab.min(panels.len().saturating_sub(1));
            quad.expanded = expanded;
        }
        drop(sel);
        self.quad_open_target = Some(target);
        self.quad_panels = panels;
        match &self.quad_open_target {
            Some(QuadTarget::Hovered { handle, sub, .. }) => {
                self.quad_refresh_subentity_highlight(i, *handle, *sub);
            }
            _ => self.quad_refresh_subentity_highlight(i, Handle::new(0), None),
        }
    }

    /// Re-classify the OPEN Hover quad at the cursor (docs/quad-menu.md
    /// §3.5.1) — per settled move and per Shift flip: the hovered entity's
    /// subentity candidates re-resolve and a changed nearest (kind or key)
    /// — or a changed entity beneath the hover highlight — rebuilds the
    /// panels + highlight immediately, no dwell, anchor kept. A cursor over
    /// the quad's own tile (no hover entity) changes nothing; the gate going
    /// off (Shift released, the setting flipped) folds the quad back to its
    /// solid level and collapses it.
    fn quad_reclassify_open_hover(&mut self, i: usize, pos: Point) {
        let (expanded, active_tab) = {
            let sel = self.tabs[i].scene.selection.borrow();
            let Some(quad) = sel.menu.quad.as_ref() else {
                return;
            };
            if quad.mode != QuadMode::Hover {
                return;
            }
            (quad.expanded, quad.active_tab)
        };
        let Some(handle) = self.tabs[i].scene.hover_highlight else {
            return;
        };
        let Some(entity) = self.tabs[i].scene.document.get_entity(handle) else {
            return;
        };
        let type_key = crate::entities::traits::entity_type_name(entity).to_string();
        let world = self.cursor_world_at(i, pos);
        let candidates = self.quad_subentity_candidates(i, handle, world);
        let nearest = candidates.first().copied();
        let changed = match self.quad_open_target.as_ref() {
            Some(QuadTarget::Hovered {
                handle: open_handle,
                sub: open_sub,
                ..
            }) => {
                let entity_changed = *open_handle != handle;
                let sub_changed = match (open_sub, &nearest) {
                    (None, None) => false,
                    (None, Some(_)) | (Some(_), None) => true,
                    (Some(a), Some(b)) => a.kind != b.kind || a.key != b.key,
                };
                entity_changed || sub_changed
            }
            _ => true,
        };
        if !changed {
            return;
        }
        let target = QuadTarget::Hovered {
            handle,
            type_key,
            sub: nearest,
        };
        let keep_expanded = expanded && nearest.is_some();
        self.quad_rebuild_hover_panels(
            i,
            target,
            candidates,
            0,
            keep_expanded,
            active_tab,
        );
    }

    /// Shift's flip re-answers the quad immediately (docs/quad-menu.md
    /// §3.5.1): the open Hover quad re-classifies at its anchor without any
    /// dwell, and a counting dwell re-runs its step — mode 1's Shift+hover
    /// open skips the delay outright; anything else keeps counting. `pos` is
    /// the resting cursor position.
    pub(in crate::app) fn quad_on_modifier_flip(&mut self, i: usize, pos: Point) {
        if self.quad_open() {
            self.quad_reclassify_open_hover(i, pos);
        } else if self.quad_hover_counting() {
            self.quad_hover_step(i, pos, 0);
        }
    }

    /// Keyboard Tab with the quad open in subentity mode
    /// (docs/quad-menu.md §3.5.1): cycle the staged candidates — faces,
    /// edges and vertices as they ranked — wrapping, each step re-resolving
    /// the primary panel + highlight instantly. `false` (no candidates
    /// staged, i.e. a solid-level quad) hands the key back so the quad keeps
    /// every other key's fall-through law.
    pub(in crate::app) fn quad_cycle_subentities(&mut self, step: isize) -> bool {
        let i = self.active_tab;
        let (candidates, index, quad_open_state) = {
            let sel = self.tabs[i].scene.selection.borrow();
            let Some(quad) = sel.menu.quad.as_ref() else {
                return false;
            };
            (
                quad.sub_candidates.clone(),
                quad.sub_index,
                (quad.expanded, quad.active_tab),
            )
        };
        if candidates.is_empty() {
            return false;
        }
        let Some(sub) = candidates.get(
            (index as isize + step).rem_euclid(candidates.len() as isize) as usize
        ) else {
            return false;
        };
        let Some(QuadTarget::Hovered {
            handle,
            type_key,
            sub: Some(_),
        }) = self.quad_open_target.clone()
        else {
            return false;
        };
        let next = (index as isize + step).rem_euclid(candidates.len() as isize) as usize;
        let target = QuadTarget::Hovered {
            handle,
            type_key,
            sub: Some(*sub),
        };
        self.quad_rebuild_hover_panels(
            i,
            target,
            candidates,
            next,
            quad_open_state.0,
            quad_open_state.1,
        );
        true
    }

    /// Open the quad cursor menu at `anchor` for `mode` (docs/quad-menu.md
    /// §3.1): classify the target, build the panels once (per-type MRU +
    /// curated tables + the visible plugins' registered quad panels) and open
    /// the window state on the selection boundary. Replaces any open context
    /// menu.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn open_quad(&mut self, mode: QuadMode, anchor: Point) -> bool {
        // The Hover quad resolves subentities at open time
        // (docs/quad-menu.md §3.5.1): the hovered face/edge/vertex under the
        // cursor — gated by `quad_acis_faces` × Shift read live here, so
        // every trigger picks it up from the current modifier state
        // (`self.shift_down`, maintained by `ModifiersChanged` →
        // `SetModifiers`). Selection / NoSelection never resolve.
        let (target, sub_candidates) = match mode {
            QuadMode::Hover => {
                let i = self.active_tab;
                let world = self.cursor_world_at(i, anchor);
                self.quad_hover_classified(i, world)
            }
            _ => (self.quad_target_for(mode), Vec::new()),
        };
        let plugin_panels = self.quad_plugin_panels();
        let shift = self.shift_down;
        let panels = quad_panels_for_target(
            &target,
            &self.quad_mru,
            self.quad_width as usize,
            &plugin_panels,
            self.quad_acis_faces,
            shift,
        );
        // Nothing to offer (an object type with no commands, an empty
        // creation catalogue): no quad, so the shortcut menu still answers
        // and Enter keeps its meaning. Any mode renders when ANY panel has
        // items: hover/selection usually fill the primary panel, while a
        // type without curated commands can still offer a plugin's tab.
        let offers = panels.iter().any(|panel| !panel.items.is_empty());
        if !offers {
            return false;
        }
        let i = self.active_tab;
        {
            let mut sel = self.tabs[i].scene.selection.borrow_mut();
            // A quad takes over the cursor: the legacy menu (state and all)
            // goes away, and a pick counts as "another interaction" for the
            // Enter-first right-click cycle.
            sel.menu.close();
            let mut quad = QuadUi::open(anchor, mode);
            quad.sub_candidates = sub_candidates.clone();
            quad.sub_index = 0;
            sel.menu.quad = Some(quad);
            sel.input.right_click_entered = false;
        }
        self.quad_open_target = Some(target);
        self.quad_panels = panels;
        self.quad_hover_since = None;
        // The resolved subentity highlights at once through the preview-wire
        // overlay (a solid-level or vertex target clears instead).
        if let Some(QuadTarget::Hovered { handle, sub, .. }) = &self.quad_open_target {
            self.quad_refresh_subentity_highlight(i, *handle, *sub);
        }
        true
    }

    /// Close the quad: drop the window state on the active tab's selection
    /// boundary (the same `SelectionMenu::close` every disruptive transition
    /// runs — quad and legacy menu are mutually exclusive) plus the built
    /// panels, the hover dwell and the subentity highlight wires
    /// (docs/quad-menu.md §3.5.1 — the preview-wire overlay belongs to the
    /// open quad; no command is active while it is). The single teardown
    /// point for every quad close, including the disrupted document tabs
    /// (`TabSwitch`): whoever tears the quad down on an app-global level
    /// must drop the app-global panel state with it. Leaves focus alone; see
    /// [`Self::close_quad_refocus`].
    pub(crate) fn close_quad(&mut self) {
        let i = self.active_tab;
        if let Some(tab) = self.tabs.get_mut(i) {
            tab.scene.selection.borrow_mut().menu.close();
            tab.scene.clear_preview_wire();
        }
        self.quad_panels.clear();
        self.quad_open_target = None;
        self.quad_hover_since = None;
    }

    /// Close the quad and hand the keyboard back to the command line — the
    /// same focus discipline `close_context_menu` applies.
    pub(crate) fn close_quad_refocus(&mut self) -> Task<Message> {
        self.close_quad();
        self.focus_cmd_input()
    }

    /// Mutate the open quad's transient UI state (expand / active tab),
    /// leaving a closed quad untouched.
    fn with_quad_ui(&mut self, f: impl FnOnce(&mut QuadUi)) {
        let i = self.active_tab;
        let mut sel = self.tabs[i].scene.selection.borrow_mut();
        if let Some(quad) = sel.menu.quad.as_mut() {
            f(quad);
        }
    }

    /// Route a quad interaction (docs/quad-menu.md §3.6); the update router
    /// forwards every `Message::Quad` here.
    pub(crate) fn on_quad_msg(&mut self, msg: QuadMsg) -> Task<Message> {
        match msg {
            QuadMsg::Toggle => {
                // Same persistence pattern as the right-click option
                // switches: flip the mirrored setting, then the settings
                // diff decides whether the config is rewritten.
                self.quad = !self.quad;
                self.persist_settings_if_changed();
                if !self.quad && self.quad_open() {
                    return self.close_quad_refocus();
                }
                Task::none()
            }
            QuadMsg::Expand(entered) => {
                self.with_quad_ui(|quad| {
                    quad.expanded = entered;
                    if !entered {
                        quad.active_tab = 0;
                    }
                });
                Task::none()
            }
            QuadMsg::Tab(index) => {
                self.with_quad_ui(|quad| {
                    quad.active_tab = index;
                    quad.expanded = true;
                });
                Task::none()
            }
            QuadMsg::Close => self.close_quad_refocus(),
            QuadMsg::Pick {
                panel_id,
                item_index,
            } => self.on_quad_pick(panel_id, item_index),
        }
    }

    /// Quad display options (docs/quad-menu.md §3.7): the QUADDISPLAY bits,
    /// `QUADWIDTH`, `QUADCOMMANDLAUNCH` and the hover delay. Every change is
    /// clamped the way `SETVAR` clamps it and persisted like the
    /// neighbouring option switches.
    pub(crate) fn on_quad_option(&mut self, option: QuadOptionMsg) -> Task<Message> {
        match option {
            QuadOptionMsg::Quad(on) => self.quad = on,
            QuadOptionMsg::OnHover(on) => self.quad_on_hover = on,
            QuadOptionMsg::OnSelect(on) => self.quad_on_select = on,
            QuadOptionMsg::OnRightClick(on) => self.quad_on_right_click = on,
            QuadOptionMsg::SuppressHoverWhenSelection(on) => {
                self.quad_suppress_hover_when_selection = on
            }
            QuadOptionMsg::Width(width) => {
                self.quad_width = crate::app::settings::clamp_quad_width(width)
            }
            QuadOptionMsg::CommandLaunch(on) => self.quad_command_launch = on,
            QuadOptionMsg::HoverDelay(ms) => {
                self.quad_hover_delay_ms =
                    crate::app::settings::clamp_quad_hover_delay_ms(ms)
            }
            QuadOptionMsg::AcisFaces(faces) => {
                self.quad_acis_faces = crate::app::settings::clamp_quad_acis_faces(faces)
            }
        }
        // Turning the quad off must not leave an open quad behind.
        if !self.quad && self.quad_open() {
            self.close_quad();
        }
        self.persist_settings_if_changed();
        Task::none()
    }

    /// Run a picked quad item. The quad closes first so the action sees the
    /// same state a typed command would; plain commands launch through the
    /// ordinary dispatch path (mirroring [`Self::on_context_menu_pick`]) and
    /// record the per-type MRU, while `quadplugin:` items go to the pick
    /// path's own host dispatch ([`Self::quad_plugin_action`]) — never the
    /// generic dispatcher, and never the MRU.
    pub(crate) fn on_quad_pick(
        &mut self,
        panel_id: String,
        item_index: usize,
    ) -> Task<Message> {
        let i = self.active_tab;
        let Some(item) = self
            .quad_panels
            .iter()
            .find(|panel| panel.id == panel_id)
            .and_then(|panel| panel.items.get(item_index))
            .cloned()
        else {
            return Task::none();
        };
        // The quad opens only on an idle drawing; a command that started
        // since (typed line, another pick) is not clobbered by a stale pick.
        if self.tabs[i].active_cmd.is_some() {
            self.close_quad();
            return Task::none();
        }
        // The target the quad was opened for decides hover-vs-selection
        // semantics; captured at open time so a live-hover change between
        // open and pick cannot rewrite what this pick does.
        let open_target = self
            .quad_open_target
            .clone()
            .unwrap_or_else(|| self.quad_target());
        // Hover quad pick on an unselected target: the command acts on the
        // hovered entity, so align the selection first (docs/quad-menu.md §3.4).
        if let QuadTarget::Hovered { handle, .. } = &open_target {
            if !self.tabs[i].scene.selected.contains(handle) {
                self.tabs[i].scene.select_entity(*handle, true);
                self.refresh_properties();
            }
        }
        // Close first, exactly like a context-menu pick — the canonical
        // teardown drops the boundary state and the app-global panel state.
        self.close_quad();
        // A pick counts as "another interaction" for the Enter-first
        // right-click cycle: the next right-click acts as Enter again.
        self.tabs[i]
            .scene
            .selection
            .borrow_mut()
            .input
            .right_click_entered = false;
        // MRU: plain commands remember against the target's type key; plugin
        // quad actions ride their own host dispatch unrecorded (they are not
        // entity commands yet).
        let is_plugin_action = item.command.starts_with(QUAD_PLUGIN_PREFIX);
        if !is_plugin_action {
            self.quad_record_mru(&open_target, &item.command);
        }
        let focus = self.focus_cmd_input();
        let task = if is_plugin_action {
            self.quad_plugin_action(&item.command)
        } else {
            self.update(Message::Command(item.command))
        };
        Task::batch(vec![focus, task])
    }

    /// Remember a plain quad command in the MRU: hovered and selection
    /// targets record under the *last* of the target's type keys (a mixed
    /// selection acts as a whole); the void records globally.
    fn quad_record_mru(&mut self, target: &QuadTarget, command: &str) {
        match target {
            QuadTarget::Hovered { type_key, .. } => self.quad_mru.record_for(type_key, command),
            QuadTarget::Selection { type_keys, .. } => {
                if let Some(type_key) = type_keys.last() {
                    self.quad_mru.record_for(type_key, command);
                }
            }
            QuadTarget::None => self.quad_mru.record_global(command),
        }
    }

    /// QUADCOMMANDLAUNCH open-and-act (docs/quad-menu.md §3.4): align the
    /// selection to the hovered entity, remember the launch in the MRU and
    /// run the entity type's default command — all without opening any menu.
    /// The teardown is the canonical [`Self::close_quad`].
    fn quad_default_launch(
        &mut self,
        handle: Handle,
        type_key: &str,
        default_cmd: &str,
    ) -> Task<Message> {
        let i = self.active_tab;
        if !self.tabs[i].scene.selected.contains(&handle) {
            self.tabs[i].scene.select_entity(handle, true);
            self.refresh_properties();
        }
        // The canonical teardown; then the usual Enter-first cycle reset
        // a right-click pick counts as.
        self.close_quad();
        self.tabs[i]
            .scene
            .selection
            .borrow_mut()
            .input
            .right_click_entered = false;
        self.quad_mru.record_for(type_key, default_cmd);
        self.update(Message::Command(default_cmd.to_string()))
    }

    /// Quad precedence for an idle right-click (docs/quad-menu.md §3.4):
    /// `Some(task)` replaces the legacy shortcut-menu path, `None` lets it
    /// proceed. A command being active keeps the legacy behaviour entirely.
    /// `held_ms` is the press duration the release path computes for the
    /// time-sensitive right-click modes (SHORTCUTMENUDURATION): a quick
    /// click over an entity launches the default command
    /// (`QUADCOMMANDLAUNCH`), a held one opens the No Selection quad
    /// instead — the way a void right-click does.
    pub(in crate::app) fn quad_right_click_takeover(
        &mut self,
        at: Point,
        held_ms: i32,
    ) -> Option<Task<Message>> {
        let i = self.active_tab;
        if self.tabs[i].active_cmd.is_some() {
            return None;
        }
        let target = self.quad_target();
        // Open-and-act: QUADCOMMANDLAUNCH makes a right-click over an entity
        // run the type's default command without offering any menu, so the
        // launch flag itself decides (docs/quad-menu.md §3.4). A held press
        // is not an open-and-act: like the void's, it opens the No
        // Selection quad — quick right-click launches, hold displays.
        if self.quad_command_launch {
            if let QuadTarget::Hovered { handle, type_key, .. } = &target {
                if held_ms >= self.right_click_hold_ms {
                    if self.open_quad(QuadMode::NoSelection, at) {
                        return Some(self.unfocus_widgets());
                    }
                    return None;
                }
                // The verb resolves the same way the quad model assembles
                // the primary tile: the type's MRU head, else the curated
                // first (docs/quad-menu.md §3.3/§3.4).
                let default_cmd = self
                    .quad_mru
                    .recent_for(type_key, 1)
                    .first()
                    .cloned()
                    .or_else(|| {
                        crate::modules::registry::quad_entity_commands(type_key)
                            .first()
                            .map(|cmd| (*cmd).to_string())
                    });
                if let Some(default_cmd) = default_cmd {
                    return Some(self.quad_default_launch(*handle, type_key, &default_cmd));
                }
                // Neither an MRU head nor a curated default: the menu
                // choices below answer.
            }
        }
        let has_selection = !self.tabs[i].scene.selected.is_empty();
        if has_selection && self.quad_on_select && self.open_quad(QuadMode::Selection, at) {
            return Some(self.unfocus_widgets());
        }
        if target.is_none() && self.quad_on_right_click && self.open_quad(QuadMode::NoSelection, at)
        {
            return Some(self.unfocus_widgets());
        }
        // Over an entity with launch off, or with the respective display
        // option off: the legacy shortcut menu answers.
        None
    }

    /// Hover-quad dwell step (docs/quad-menu.md §3.4, hover variant). Called
    /// from the per-move hover block (`rest_ms = 0`: the cursor only just
    /// moved) and from the settled-rollover tick (the dwell's own rest so
    /// far). Qualifying rest arms and counts; past the configured delay with
    /// no quad open, the collapsed quad appears at the cursor — without
    /// touching the selection.
    pub(in crate::app) fn quad_hover_step(&mut self, i: usize, pos: Point, rest_ms: u64) {
        let Some(tab) = self.tabs.get(i) else {
            return;
        };
        // A command, a grip edit or a drag owns the pointer; the hover quad
        // counts only on the idle rollover.
        let (running, dragging, quad_state) = {
            let sel = tab.scene.selection.borrow();
            (
                tab.active_cmd.is_some() || tab.active_grip.is_some(),
                sel.input.left_dragging || sel.input.right_dragging || sel.input.middle_down,
                sel.menu.quad.clone(),
            )
        };
        if running || dragging {
            return;
        }
        // A quad that is open follows its anchor for the Hover mode only:
        // a vanished target or a drift past [`QUAD_DRIFT_PX`] closes it, and
        // a Selection / NoSelection quad stays out of hover accounting.
        if let Some(quad) = &quad_state {
            if quad.mode == QuadMode::Hover {
                // The tile sits up and right of the anchor: the cursor may
                // cross from the object to it (losing the hover on the way)
                // without closing it; anywhere else closes it.
                use crate::ui::popup::quad_overlay::{QUAD_HOVER_GAP, QUAD_TILE};
                let reach = QUAD_HOVER_GAP + QUAD_TILE + QUAD_DRIFT_PX;
                let (dx, dy) = (pos.x - quad.anchor.x, quad.anchor.y - pos.y);
                let toward_tile = (-QUAD_DRIFT_PX..=reach).contains(&dx)
                    && (-QUAD_DRIFT_PX..=reach).contains(&dy);
                if !toward_tile {
                    self.close_quad();
                } else {
                    // Live subentity re-classification while open
                    // (docs/quad-menu.md §3.5.1): the nearest face/edge/
                    // vertex is recomputed on every settled move — a changed
                    // nearest (or entity) rebuilds the panels + highlight at
                    // once, no dwell, anchor kept.
                    self.quad_reclassify_open_hover(i, pos);
                }
            }
            return;
        }
        let qualifies = self.quad
            && self.quad_on_hover
            && tab.scene.hover_highlight.is_some()
            && (tab.scene.selected.is_empty() || !self.quad_suppress_hover_when_selection);
        if !qualifies {
            self.quad_hover_since = None;
            return;
        }
        // Shift+hover over a face-bearing type skips the dwell outright
        // (mode 1 — docs/quad-menu.md §3.5.1): pressing Shift while the
        // cursor rests on the solid must answer without waiting the delay
        // out. Mode 2 resolves subentities on the regular delay instead.
        let immediate = self.quad_acis_faces == 1
            && self.shift_down
            && tab.scene.hover_highlight.is_some_and(|handle| {
                tab.scene.document.get_entity(handle).is_some_and(|entity| {
                    let type_key = crate::entities::traits::entity_type_name(entity);
                    crate::modules::registry::QUAD_ACIS_FACE_TYPES
                        .iter()
                        .any(|k| k.eq_ignore_ascii_case(type_key))
                })
            });
        let due = match self.quad_hover_since {
            Some(since) => {
                (since.elapsed().as_millis() as u64).max(rest_ms) >= self.quad_hover_delay_ms
            }
            // A fresh qualifying rest: the idle rollover itself only reports
            // after its own dwell, so an already-rested cursor can be due at
            // this first settle.
            None => {
                let already_due = rest_ms >= self.quad_hover_delay_ms;
                self.quad_hover_since = Some(std::time::Instant::now());
                already_due
            }
        };
        if due || immediate {
            self.quad_hover_since = None;
            let _ = self.open_quad(QuadMode::Hover, pos);
        }
    }

    /// Enter while the quad is open: pick the primary panel's first item
    /// when it has one (`quad.primary` for hover/selection quads, the Recent
    /// panel for the No Selection quad).
    fn on_quad_enter_pick(&mut self) -> Task<Message> {
        let Some(primary) = self.quad_panels.first().cloned() else {
            return Task::none();
        };
        if primary.items.is_empty() {
            return Task::none();
        }
        self.on_quad_pick(primary.id.clone(), 0)
    }

    /// Tab-cycle the open quad's panels (keyboard Tab support): the next /
    /// previous panel tab becomes active, expanding the quad. `active_tab`
    /// indexes ALL panels (0 = the primary), so the cycle wraps through the
    /// primary panel and every group tab reaches itself; with no panel
    /// beyond the primary there is no tab to cycle (returns `false`), so
    /// the key falls through.
    pub(in crate::app) fn quad_cycle_tabs(&mut self, step: isize) -> bool {
        let count = self.quad_panels.len();
        if count <= 1 {
            return false;
        }
        let i = self.active_tab;
        let mut sel = self.tabs[i].scene.selection.borrow_mut();
        let Some(quad) = sel.menu.quad.as_mut() else {
            return false;
        };
        let current = if quad.expanded { quad.active_tab } else { 0 };
        quad.expanded = true;
        quad.active_tab = (current as isize + step).rem_euclid(count as isize) as usize;
        true
    }

    /// Keyboard while the quad is open (docs/quad-menu.md §3.6): Escape
    /// closes, Enter picks the primary item, Tab cycles the tabs and the
    /// digits 1–9 pick the primary panel's items. Anything else closes the
    /// quad and falls through, exactly as [`Self::intercept_context_menu_key`]
    /// yields for non-menu keys.
    pub(in crate::app) fn intercept_quad_key(&mut self, msg: &Message) -> Option<Task<Message>> {
        let base = match msg {
            Message::ShortcutPressed(key) => key.rsplit('+').next(),
            _ => None,
        };
        if matches!(msg, Message::CommandEscape) || base == Some("ESCAPE") {
            return Some(self.close_quad_refocus());
        }
        if !self.quad_open() {
            // A quad closed since the gate tested it (a repeat of a global
            // shortcut): nothing left to drive.
            return None;
        }
        // The hover quad opens without being asked for: a key typed then
        // (Enter to repeat the last command, an option letter, a digit) is
        // meant for the command line, so the quad closes and lets it through.
        let hover = self.tabs[self.active_tab]
            .scene
            .selection
            .borrow()
            .menu
            .quad
            .as_ref()
            .is_some_and(|quad| quad.mode == QuadMode::Hover);
        let key = matches!(
            msg,
            Message::ShortcutPressed(_)
                | Message::DynTabNext
                | Message::DynTabPrev
                | Message::CommandFinalize
                | Message::CommandSpace
                | Message::CommandSubmit
                | Message::CommandAppendChar(_)
                | Message::CommandInput(_)
        );
        if hover && key {
            // The Shift+hover subentity quad owns the keyboard Tab while
            // candidates are staged (docs/quad-menu.md §3.5.1): cycling
            // faces → edges → vertices instead of closing. Without the
            // resolution the hover quad keeps the fall-through law above
            // (the VIEW tab-strip emission, `QuadMsg::Tab`, is untouched —
            // the Selection / NoSelection quads below keep cycling panels).
            let step = match msg {
                Message::DynTabNext => Some(1),
                Message::DynTabPrev => Some(-1),
                _ => None,
            };
            if let Some(step) = step {
                if self.quad_cycle_subentities(step) {
                    return Some(Task::none());
                }
            }
            self.close_quad();
            return None;
        }
        match msg {
            Message::DynTabNext => {
                if self.quad_cycle_tabs(1) {
                    Some(Task::none())
                } else {
                    None
                }
            }
            Message::DynTabPrev => {
                if self.quad_cycle_tabs(-1) {
                    Some(Task::none())
                } else {
                    None
                }
            }
            Message::CommandFinalize | Message::CommandSpace => {
                if self.command_line.input.trim().is_empty() {
                    Some(self.on_quad_enter_pick())
                } else {
                    // Typed text pending: Enter runs it, the quad just
                    // closes (mirrors the context-menu interception).
                    let close = self.close_quad_refocus();
                    let run = self.update(Message::CommandFinalize);
                    Some(Task::batch(vec![close, run]))
                }
            }
            Message::CommandSubmit => {
                if self.command_line.input.trim().is_empty() {
                    Some(self.on_quad_enter_pick())
                } else {
                    let _ = self.close_quad_refocus();
                    None
                }
            }
            Message::CommandAppendChar(s) => self.quad_typed(s.chars().next()),
            Message::CommandInput(buf) => {
                // The focused command-line field delivers the whole buffer;
                // only a single appended character is a digit candidate.
                let appended = buf
                    .strip_prefix(self.command_line.input.as_str())
                    .filter(|rest| rest.chars().count() == 1)
                    .and_then(|rest| rest.chars().next());
                match appended {
                    Some(ch) => self.quad_typed(Some(ch)),
                    None => {
                        let _ = self.close_quad_refocus();
                        None
                    }
                }
            }
            _ => None,
        }
    }

    /// A printable key while the quad is open: 1–9 pick the primary panel's
    /// items; anything else closes the quad and lets the key fall through to
    /// the command line. Pending command-line text owns the key instead: the
    /// quad closes and the key falls through, never swallowed by a digit
    /// pick (the same guard the Enter branches apply).
    fn quad_typed(&mut self, ch: Option<char>) -> Option<Task<Message>> {
        let ch = ch?;
        if !self.command_line.input.trim().is_empty() {
            let _ = self.close_quad_refocus();
            return None;
        }
        let digit = ch.to_digit(10)?;
        if !(1..=9).contains(&digit) {
            let _ = self.close_quad_refocus();
            return None;
        }
        if !self.quad_open() {
            return None;
        }
        let item_index = digit as usize - 1;
        let Some(primary) = self.quad_panels.first().cloned() else {
            return None;
        };
        if primary.items.is_empty() || item_index >= primary.items.len() {
            return None;
        }
        Some(self.on_quad_pick(primary.id, item_index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::settings::RightClickMode;
    use crate::command::StepInput;
    use crate::ui::popup::context_menu::MenuAction;
    use glam::DVec3;
    use iced::Point;

    /// `update` recursion (right-click → CommandFinalize → CommandSubmit →
    /// apply) is deeper than the default test-thread stack; production
    /// threads are sized for it (see `.cargo/config.toml`), so tests get an
    /// equivalent stack here.
    fn with_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(f)
            .expect("spawn test thread")
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }

    fn app() -> OpenCADStudio {
        let mut app = OpenCADStudio::new_for_test();
        app.automation_op(r#"{"op":"new"}"#);
        app.tabs[0].scene.selection.borrow_mut().view.vp_size = (800.0, 600.0);
        let _ = app.update(Message::ViewportMove(Point::new(200.0, 150.0)));
        app
    }

    fn start(app: &mut OpenCADStudio, cmd: &str, points: &[(f64, f64)]) {
        let _ = app.dispatch_command(cmd);
        for &(x, y) in points {
            let _ = app.feed_command(StepInput::Point(DVec3::new(x, y, 0.0)));
        }
    }

    fn right_click(app: &mut OpenCADStudio) {
        let _ = app.update(Message::ViewportRightPress);
        let _ = app.update(Message::ViewportRightRelease);
    }

    fn menu_open(app: &OpenCADStudio) -> bool {
        app.tabs[0].scene.selection.borrow().menu.open_at.is_some()
    }

    fn active(app: &OpenCADStudio) -> Option<&'static str> {
        app.tabs[0].active_cmd.as_ref().map(|c| c.name())
    }

    fn mspace_zoom_app() -> (OpenCADStudio, Handle) {
        let mut app = OpenCADStudio::new_for_test();
        app.automation_op(r#"{"op":"new"}"#);
        let scene = &mut app.tabs[0].scene;
        scene.document.add_layout("Zoom").unwrap();
        scene.set_current_layout("Zoom".into());
        scene.selection.borrow_mut().view.vp_size = (800.0, 600.0);
        let mut viewport = codec::entities::Viewport::new();
        viewport.id = 2;
        viewport.center = codec::types::Vector3::new(150.0, 100.0, 0.0);
        viewport.width = 200.0;
        viewport.height = 100.0;
        viewport.view_height = 200.0;
        viewport.custom_scale = 0.5;
        viewport.view_direction = codec::types::Vector3::UNIT_Z;
        viewport.turn_on();
        let handle = scene.add_entity(codec::EntityType::Viewport(Box::new(viewport)));
        scene.active_viewport = Some(handle);
        scene.normalize_active_viewport_view();
        (app, handle)
    }

    #[test]
    fn mspace_zoom_window_preserves_paper_view() {
        with_stack(|| {
            // Bare ZOOM mouse corners, the Window prompt option, and the
            // ribbon/context-menu command all reach the same result handler.
            for command in ["ZOOM", "ZOOM W", "ZOOM WINDOW", "ZOOM prompt W"] {
                let (mut app, handle) = mspace_zoom_app();
                let paper = app.tabs[0].scene.camera.borrow().clone();
                let _ = app.dispatch_command(if command == "ZOOM prompt W" {
                    "ZOOM"
                } else {
                    command
                });
                if command == "ZOOM prompt W" {
                    let _ = app.feed_active_cmd("W");
                }
                // A wide window must use the viewport's 2:1 aspect, not
                // the canvas's 4:3 aspect: width 80 needs view height 40.
                let _ = app.feed_command(StepInput::Point(DVec3::new(10.0, 20.0, 0.0)));
                let _ = app.feed_command(StepInput::Point(DVec3::new(90.0, 40.0, 0.0)));
                let scene = &app.tabs[0].scene;
                let camera = scene.camera.borrow();
                assert_eq!(camera.target, paper.target, "{command}: paper panned");
                assert_eq!(camera.distance, paper.distance, "{command}: paper zoomed");
                assert_eq!(camera.rotation, paper.rotation);
                assert_eq!(camera.projection, paper.projection);
                assert_eq!(camera.fov_y, paper.fov_y);
                assert_eq!(camera.lens, paper.lens);
                assert_eq!(camera.depth_half_range, paper.depth_half_range);
                assert_eq!(camera.model_bounds, paper.model_bounds);
                let Some(codec::EntityType::Viewport(viewport)) = scene.document.get_entity(handle)
                else {
                    panic!("missing viewport");
                };
                assert!((viewport.view_target.x - 50.0).abs() < 1e-5);
                assert!((viewport.view_target.y - 30.0).abs() < 1e-5);
                assert_eq!(viewport.view_center.x, 0.0);
                assert_eq!(viewport.view_center.y, 0.0);
                assert!((viewport.view_height - 40.0).abs() < 1e-4);
                assert!((viewport.custom_scale - 2.5).abs() < 1e-5);
                drop(camera);
                let _ = app.dispatch_command("ZOOM PREVIOUS");
                let camera = app.tabs[0].scene.camera.borrow();
                assert_eq!(camera.target, paper.target);
                assert_eq!(camera.distance, paper.distance);
                let Some(codec::EntityType::Viewport(viewport)) =
                    app.tabs[0].scene.document.get_entity(handle)
                else {
                    panic!("missing viewport");
                };
                assert!((viewport.view_height - 200.0).abs() < 1e-4);
            }
        });
    }

    #[test]
    fn mspace_zoom_factors_preserve_paper_view() {
        with_stack(|| {
            for (command, height) in [
                ("ZOOM SCALE 2", 100.0),
                ("ZOOM SCALE 2X", 100.0),
                ("ZOOM SCALE 100X", 2.0),
                ("ZOOM SCALE 2XP", 50.0),
                ("ZOOM IN", 200.0 / 1.5),
                ("ZOOM OUT", 300.0),
            ] {
                let (mut app, handle) = mspace_zoom_app();
                let paper = app.tabs[0].scene.camera.borrow().clone();
                let _ = app.dispatch_command(command);
                let scene = &app.tabs[0].scene;
                let camera = scene.camera.borrow();
                assert_eq!(camera.target, paper.target, "{command}: paper panned");
                assert_eq!(camera.distance, paper.distance, "{command}: paper zoomed");
                let Some(codec::EntityType::Viewport(viewport)) = scene.document.get_entity(handle)
                else {
                    panic!("missing viewport");
                };
                assert!((viewport.view_height - height).abs() < 1e-3, "{command}");
                assert!((viewport.custom_scale - 100.0 / height).abs() < 1e-5);
            }
        });
    }

    fn menu_actions(app: &OpenCADStudio) -> Vec<MenuAction> {
        app.current_context_menu()
            .selectable()
            .into_iter()
            .map(|s| s.action)
            .collect()
    }

    #[test]
    fn shortcut_menu_mode_single_rmb_opens_menu_during_command() {
        with_stack(|| {
            let mut app = app();
            assert_eq!(app.right_click_mode, RightClickMode::ShortcutMenu);
            start(&mut app, "LINE", &[(0.0, 0.0), (10.0, 0.0)]);
            right_click(&mut app);
            assert!(menu_open(&app));
            assert_eq!(active(&app), Some("LINE"));
            let acts = menu_actions(&app);
            assert_eq!(acts[0], MenuAction::Enter);
            assert_eq!(acts[1], MenuAction::Cancel);
            assert!(acts.contains(&MenuAction::Option("C".into())));
            assert!(acts.contains(&MenuAction::Option("U".into())));
            assert!(acts.contains(&MenuAction::ToggleSubmenu(SubmenuId::SnapOverrides)));
            assert!(acts.contains(&MenuAction::Command("'PAN".into())));
        });
    }

    #[test]
    fn enter_first_mode_first_rmb_is_enter_second_opens_menu() {
        with_stack(|| {
            let mut app = app();
            app.right_click_mode = RightClickMode::EnterFirst;
            start(&mut app, "LINE", &[(0.0, 0.0), (10.0, 0.0)]);
            right_click(&mut app);
            // Enter ends LINE.
            assert!(!menu_open(&app));
            assert_eq!(active(&app), None);
            // Idle: the next right-click opens the menu.
            right_click(&mut app);
            assert!(menu_open(&app));
            let acts = menu_actions(&app);
            assert_eq!(
                acts[0],
                MenuAction::Command("LINE".into()),
                "Repeat LINE first"
            );
        });
    }

    #[test]
    fn time_sensitive_quick_click_is_enter_and_hold_opens_menu() {
        with_stack(|| {
            let mut app = app();
            app.right_click_mode = RightClickMode::TimeSensitive;
            app.right_click_hold_ms = 250;
            start(&mut app, "PLINE", &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
            // Held: opens the menu, PLINE keeps running.
            let _ = app.update(Message::ViewportRightPress);
            {
                let mut sel = app.tabs[0].scene.selection.borrow_mut();
                sel.input.right_press_time =
                    iced::time::Instant::now().checked_sub(std::time::Duration::from_millis(400));
            }
            let _ = app.update(Message::ViewportRightRelease);
            assert!(menu_open(&app));
            assert_eq!(active(&app), Some("PLINE"));
            // Dismiss, then a quick click finishes the polyline.
            let _ = app.update(Message::CommandEscape);
            assert!(!menu_open(&app));
            assert_eq!(active(&app), Some("PLINE"));
            right_click(&mut app);
            assert!(!menu_open(&app));
            assert_eq!(active(&app), None);
        });
    }

    #[test]
    fn pending_text_runs_as_enter_in_all_modes() {
        with_stack(|| {
            for mode in RightClickMode::ALL {
                let mut app = app();
                app.right_click_mode = mode;
                start(&mut app, "LINE", &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
                app.command_line.input = "C".into();
                right_click(&mut app);
                assert!(!menu_open(&app), "{mode:?}");
                assert_eq!(active(&app), None, "{mode:?}: Close should end LINE");
                let lines = app.tabs[0]
                    .scene
                    .document
                    .entities()
                    .filter(|e| matches!(e, codec::EntityType::Line(_)))
                    .count();
                assert_eq!(lines, 3, "{mode:?}");
            }
        });
    }

    #[test]
    fn context_menu_pick_option_feeds_keyword() {
        with_stack(|| {
            let mut app = app();
            start(&mut app, "PLINE", &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
            right_click(&mut app);
            assert!(menu_open(&app));
            let _ = app.update(Message::ContextMenuPick(MenuAction::Option("C".into())));
            assert!(!menu_open(&app));
            assert_eq!(active(&app), None);
            let closed = app.tabs[0].scene.document.entities().any(|e| {
                matches!(e, codec::EntityType::LwPolyline(p) if p.is_closed && p.vertices.len() == 3)
            });
            assert!(closed, "PLINE should have been closed by the menu pick");
        });
    }

    #[test]
    fn context_menu_pick_closes_menu_and_resets_enter_cycle() {
        with_stack(|| {
            let mut app = app();
            app.right_click_mode = RightClickMode::EnterFirst;
            start(&mut app, "LINE", &[(0.0, 0.0), (10.0, 0.0)]);
            right_click(&mut app); // Enter → LINE ends
            right_click(&mut app); // idle → menu
            assert!(menu_open(&app));
            let _ = app.update(Message::ContextMenuPick(MenuAction::Command("LINE".into())));
            assert!(!menu_open(&app));
            assert_eq!(active(&app), Some("LINE"));
            assert!(
                !app.tabs[0]
                    .scene
                    .selection
                    .borrow()
                    .input
                    .right_click_entered
            );
        });
    }

    #[test]
    fn menu_enter_with_no_highlight_picks_default() {
        with_stack(|| {
            let mut app = app();
            start(&mut app, "PLINE", &[(0.0, 0.0), (10.0, 0.0)]);
            right_click(&mut app);
            let _ = app.update(Message::CommandFinalize);
            assert!(!menu_open(&app));
            assert_eq!(active(&app), None, "default row (Done) finishes PLINE");
        });
    }

    #[test]
    fn menu_arrow_down_then_enter_picks_cancel() {
        with_stack(|| {
            let mut app = app();
            start(&mut app, "LINE", &[(0.0, 0.0), (10.0, 0.0)]);
            right_click(&mut app);
            let _ = app.update(Message::ContextMenuNavigate(ContextMenuNav::Down)); // Enter row
            assert_eq!(
                app.tabs[0].scene.selection.borrow().menu.ui.highlighted,
                Some(0)
            );
            let _ = app.update(Message::ContextMenuNavigate(ContextMenuNav::Down)); // Cancel
            let _ = app.update(Message::ContextMenuNavigate(ContextMenuNav::Enter));
            assert!(!menu_open(&app));
            assert_eq!(active(&app), None);
        });
    }

    #[test]
    fn menu_mnemonic_c_closes_pline() {
        with_stack(|| {
            let mut app = app();
            start(&mut app, "PLINE", &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
            right_click(&mut app);
            let _ = app.update(Message::CommandAppendChar("c".into()));
            assert!(!menu_open(&app));
            assert_eq!(active(&app), None);
            let closed = app.tabs[0]
                .scene
                .document
                .entities()
                .any(|e| matches!(e, codec::EntityType::LwPolyline(p) if p.is_closed));
            assert!(closed);
        });
    }

    #[test]
    fn menu_unmatched_char_closes_menu_and_types() {
        with_stack(|| {
            let mut app = app();
            app.dyn_input = false;
            start(&mut app, "LINE", &[(0.0, 0.0)]);
            right_click(&mut app);
            assert!(menu_open(&app));
            let _ = app.update(Message::CommandAppendChar("7".into()));
            assert!(!menu_open(&app));
            assert_eq!(active(&app), Some("LINE"));
            assert!(
                app.command_line.input.contains('7'),
                "{:?}",
                app.command_line.input
            );
        });
    }

    #[test]
    fn recent_input_pick_refeeds_token() {
        with_stack(|| {
            let mut app = app();
            start(&mut app, "LINE", &[(0.0, 0.0)]);
            let _ = app.feed_active_cmd("10,10");
            assert_eq!(app.command_line.recent_inputs, vec!["10,10".to_string()]);
            let _ = app.feed_active_cmd("20,0");
            right_click(&mut app);
            let _ = app.update(Message::ContextMenuSubmenuToggle(SubmenuId::RecentInput));
            let acts = menu_actions(&app);
            assert!(acts.contains(&MenuAction::FeedInput("10,10".into())));
            let _ = app.update(Message::ContextMenuPick(MenuAction::FeedInput(
                "10,10".into(),
            )));
            assert!(!menu_open(&app));
            let lines = app.tabs[0]
                .scene
                .document
                .entities()
                .filter(|e| matches!(e, codec::EntityType::Line(_)))
                .count();
            assert_eq!(lines, 3);
        });
    }

    #[test]
    fn idle_selection_menu_erase_deletes_selection() {
        with_stack(|| {
            let mut app = app();
            start(&mut app, "LINE", &[(0.0, 0.0), (10.0, 0.0)]);
            let _ = app.update(Message::CommandEscape);
            let handle = app.tabs[0]
                .scene
                .document
                .entities()
                .find_map(|e| match e {
                    codec::EntityType::Line(l) => Some(l.common.handle),
                    _ => None,
                })
                .expect("a line");
            app.tabs[0].scene.select_entities(&[handle]);
            right_click(&mut app);
            let acts = menu_actions(&app);
            assert!(acts.contains(&MenuAction::DeleteSelected));
            assert!(acts.contains(&MenuAction::Command("ROTATE".into())));
            let _ = app.update(Message::ContextMenuPick(MenuAction::DeleteSelected));
            assert!(!menu_open(&app));
            let lines = app.tabs[0]
                .scene
                .document
                .entities()
                .filter(|e| matches!(e, codec::EntityType::Line(_)))
                .count();
            assert_eq!(lines, 0);
        });
    }

    // ── Grip menu ─────────────────────────────────────────────────────

    /// A drawing with one line, selected, its start grip hot at the origin.
    fn grip_app() -> (OpenCADStudio, Handle) {
        use codec::{entities::Line, types::Vector3, EntityType};
        let mut app = app();
        app.snapper.snap_enabled = false;
        app.snapper.grid_snap_on = false;
        app.snapper.otrack_enabled = false;
        app.ortho_mode = false;
        app.polar_mode = false;
        let handle = app.tabs[0]
            .scene
            .add_entity(EntityType::Line(Line::from_points(
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(10.0, 0.0, 0.0),
            )));
        app.tabs[0].scene.selected.insert(handle);
        app.refresh_selected_grips();
        app.tabs[0].active_grip = Some(crate::scene::pick::grip::GripEdit::single(
            handle,
            0,
            false,
            DVec3::ZERO,
        ));
        (app, handle)
    }

    fn move_cursor_to(app: &mut OpenCADStudio, world: DVec3) {
        let cursor = app.tabs[0]
            .scene
            .camera
            .borrow()
            .project(
                world,
                iced::Rectangle::with_size(iced::Size::new(800.0, 600.0)),
            )
            .unwrap();
        let _ = app.on_viewport_move(Point::new(cursor.x, cursor.y));
    }

    fn line_start(app: &OpenCADStudio, handle: Handle) -> (f64, f64) {
        match app.tabs[0].scene.document.get_entity(handle) {
            Some(codec::EntityType::Line(l)) => (l.start.x, l.start.y),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rmb_on_hot_grip_opens_grip_menu() {
        with_stack(|| {
            let (mut app, _) = grip_app();
            right_click(&mut app);
            assert!(menu_open(&app));
            let acts = menu_actions(&app);
            assert_eq!(acts[0], MenuAction::Enter);
            assert!(acts.contains(&MenuAction::Grip(GripMenuCmd::Move)));
            assert!(acts.contains(&MenuAction::Grip(GripMenuCmd::Exit)));
            assert!(
                app.tabs[0].active_grip.is_some(),
                "the grip stays hot under the menu"
            );
        });
    }

    #[test]
    fn grip_menu_move_dispatches_move_with_grip_base() {
        with_stack(|| {
            let (mut app, _) = grip_app();
            right_click(&mut app);
            let _ = app.update(Message::ContextMenuPick(MenuAction::Grip(
                GripMenuCmd::Move,
            )));
            assert!(!menu_open(&app));
            assert!(app.tabs[0].active_grip.is_none());
            assert_eq!(active(&app), Some("MOVE"));
            let prompt = app.tabs[0]
                .active_cmd
                .as_ref()
                .map(|c| c.prompt())
                .unwrap_or_default();
            let base_prompt = crate::tr!("command-move", "base", count = 1i64);
            assert_ne!(prompt, base_prompt, "base already consumed");
            assert_eq!(app.last_point, Some(DVec3::ZERO));
        });
    }

    #[test]
    fn grip_menu_exit_restores_original_geometry() {
        with_stack(|| {
            let (mut app, handle) = grip_app();
            move_cursor_to(&mut app, DVec3::new(3.0, 4.0, 0.0));
            let (x, y) = line_start(&app, handle);
            assert!(
                (x - 3.0).abs() < 1e-3 && (y - 4.0).abs() < 1e-3,
                "drag applied"
            );
            right_click(&mut app);
            let _ = app.update(Message::ContextMenuPick(MenuAction::Grip(
                GripMenuCmd::Exit,
            )));
            assert!(app.tabs[0].active_grip.is_none());
            let (x, y) = line_start(&app, handle);
            assert!(
                x.abs() < 1e-9 && y.abs() < 1e-9,
                "Exit restores the original"
            );
        });
    }

    #[test]
    fn grip_menu_undo_snaps_back_but_stays_hot() {
        with_stack(|| {
            let (mut app, handle) = grip_app();
            move_cursor_to(&mut app, DVec3::new(3.0, 4.0, 0.0));
            right_click(&mut app);
            let _ = app.update(Message::ContextMenuPick(MenuAction::Grip(
                GripMenuCmd::Undo,
            )));
            let (x, y) = line_start(&app, handle);
            assert!(x.abs() < 1e-9 && y.abs() < 1e-9);
            let grip = app.tabs[0].active_grip.as_ref().expect("grip still hot");
            assert_eq!(grip.last_world, grip.origin_world);
        });
    }

    #[test]
    fn grip_menu_copy_leaves_original_and_adds_copy() {
        with_stack(|| {
            let (mut app, handle) = grip_app();
            right_click(&mut app);
            let _ = app.update(Message::ContextMenuPick(MenuAction::Grip(
                GripMenuCmd::CopyToggle,
            )));
            assert!(app.tabs[0].grip_copy);
            move_cursor_to(&mut app, DVec3::new(3.0, 4.0, 0.0));
            let _ = app.update(Message::CommandFinalize);
            let lines: Vec<(f64, f64)> = app.tabs[0]
                .scene
                .document
                .entities()
                .filter_map(|e| match e {
                    codec::EntityType::Line(l) => Some((l.start.x, l.start.y)),
                    _ => None,
                })
                .collect();
            assert_eq!(lines.len(), 2, "{lines:?}");
            let (x, y) = line_start(&app, handle);
            assert!(x.abs() < 1e-9 && y.abs() < 1e-9, "original untouched");
            assert!(lines
                .iter()
                .any(|&(x, y)| (x - 3.0).abs() < 1e-3 && (y - 4.0).abs() < 1e-3));
            assert!(
                app.tabs[0].active_grip.is_some(),
                "grip re-armed for the next copy"
            );
            // One undo step removes the copy again.
            app.undo_steps(1);
            let count = app.tabs[0]
                .scene
                .document
                .entities()
                .filter(|e| matches!(e, codec::EntityType::Line(_)))
                .count();
            assert_eq!(count, 1);
        });
    }

    #[test]
    fn grip_menu_base_point_rebases_without_placing() {
        with_stack(|| {
            let (mut app, handle) = grip_app();
            right_click(&mut app);
            let _ = app.update(Message::ContextMenuPick(MenuAction::Grip(
                GripMenuCmd::BasePoint,
            )));
            assert!(app.tabs[0].grip_base_pending);
            move_cursor_to(&mut app, DVec3::new(1.0, 1.0, 0.0));
            let _ = app.update(Message::ViewportLeftPress);
            let _ = app.update(Message::ViewportLeftRelease);
            let grip = app.tabs[0].active_grip.as_ref().expect("grip still hot");
            assert_eq!(grip.origin_world, grip.last_world);
            assert!(!app.tabs[0].grip_base_pending);
            let (x, y) = line_start(&app, handle);
            assert!((x - 1.0).abs() < 1e-3 && (y - 1.0).abs() < 1e-3);
        });
    }
}

#[cfg(test)]
mod transparent_tests {
    use super::*;
    use crate::command::StepInput;
    use glam::DVec3;

    fn with_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(f)
            .expect("spawn test thread")
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }

    fn line_app() -> OpenCADStudio {
        let mut app = OpenCADStudio::new_for_test();
        app.automation_op(r#"{"op":"new"}"#);
        app.tabs[0].scene.selection.borrow_mut().view.vp_size = (800.0, 600.0);
        let _ = app.dispatch_command("LINE");
        let _ = app.feed_command(StepInput::Point(DVec3::ZERO));
        app
    }

    fn active(app: &OpenCADStudio) -> Option<&'static str> {
        app.tabs[0].active_cmd.as_ref().map(|c| c.name())
    }

    #[test]
    fn transparent_pan_keeps_the_command_running() {
        with_stack(|| {
            let mut app = line_app();
            let _ = app.update(Message::ContextMenuPick(MenuAction::Command("'PAN".into())));
            assert!(app.tabs[0].pan_mode);
            assert_eq!(active(&app), Some("LINE"));
            // Esc leaves pan mode; the line is still being drawn.
            let _ = app.update(Message::CommandEscape);
            assert!(!app.tabs[0].pan_mode);
            assert_eq!(active(&app), Some("LINE"));
        });
    }

    #[test]
    fn transparent_zoom_window_resumes_the_command() {
        with_stack(|| {
            let mut app = line_app();
            let _ = app.dispatch_command("'ZOOM W");
            assert_eq!(active(&app), Some("ZOOM WINDOW"));
            assert!(app.tabs[0].transparent_resume);
            let _ = app.feed_command(StepInput::Point(DVec3::new(-5.0, -5.0, 0.0)));
            let _ = app.feed_command(StepInput::Point(DVec3::new(5.0, 5.0, 0.0)));
            assert_eq!(active(&app), Some("LINE"), "LINE resumes after the zoom");
            assert!(!app.tabs[0].transparent_resume);
            assert!(app.tabs[0].suspended_cmd.is_none());
            // The line still has its first point: the next pick draws a segment.
            let _ = app.feed_command(StepInput::Point(DVec3::new(10.0, 0.0, 0.0)));
            let lines = app.tabs[0]
                .scene
                .document
                .entities()
                .filter(|e| matches!(e, codec::EntityType::Line(_)))
                .count();
            assert_eq!(lines, 1);
        });
    }

    #[test]
    fn zoom_prompt_handoff_and_escape_both_resume() {
        with_stack(|| {
            let mut app = line_app();
            let _ = app.dispatch_command("'ZOOM");
            let _ = app.feed_active_cmd("E"); // → ZOOM EXTENTS, one-shot
            assert_eq!(active(&app), Some("LINE"));
            let _ = app.dispatch_command("'ZOOM");
            assert_eq!(active(&app), Some("ZOOM WINDOW"));
            let _ = app.update(Message::CommandEscape);
            assert_eq!(active(&app), Some("LINE"));
            assert!(!app.tabs[0].transparent_resume);
        });
    }

    #[test]
    fn new_command_drops_a_parked_one() {
        with_stack(|| {
            let mut app = line_app();
            let _ = app.dispatch_command("'ZOOM");
            let _ = app.dispatch_command("CIRCLE");
            assert_eq!(active(&app), Some("CIRCLE"));
            assert!(app.tabs[0].suspended_cmd.is_none());
            let _ = app.update(Message::CommandEscape);
            assert_eq!(active(&app), None, "no stale LINE comes back");
        });
    }

    #[test]
    fn snap_override_none_is_one_shot() {
        with_stack(|| {
            let mut app = line_app();
            app.snapper.snap_enabled = true;
            let _ = app.update(Message::ContextMenuPick(MenuAction::SnapOverrideNone));
            assert!(!app.snapper.snap_enabled);
            app.snapper.clear_override();
            assert!(app.snapper.snap_enabled);
        });
    }

    #[test]
    fn test_direct_distance_entry_dyn_off() {
        with_stack(|| {
            let mut app = line_app();
            app.dyn_input = false;
            assert_eq!(active(&app), Some("LINE"));

            // Point cursor in +X direction from origin
            app.tabs[0].last_cursor_world = glam::DVec3::new(10.0, 0.0, 0.0);

            // Type "50" into command line and submit
            app.command_line.input = "50".to_string();
            let _ = app.on_command_submit();

            // Line segment should be created from (0,0) to (50,0)
            let lines: Vec<_> = app.tabs[0]
                .scene
                .document
                .entities()
                .filter_map(|e| match e {
                    codec::EntityType::Line(l) => Some(l.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(lines.len(), 1);
            assert!((lines[0].start.x - 0.0).abs() < 1e-6 && (lines[0].start.y - 0.0).abs() < 1e-6);
            assert!((lines[0].end.x - 50.0).abs() < 1e-6 && (lines[0].end.y - 0.0).abs() < 1e-6);

            // Command should still be active for the next line segment
            assert_eq!(active(&app), Some("LINE"));

            // Point cursor in +Y direction from (50, 0)
            app.tabs[0].last_cursor_world = glam::DVec3::new(50.0, 20.0, 0.0);

            // Type "25" and submit
            app.command_line.input = "25".to_string();
            let _ = app.on_command_submit();

            let lines: Vec<_> = app.tabs[0]
                .scene
                .document
                .entities()
                .filter_map(|e| match e {
                    codec::EntityType::Line(l) => Some(l.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(lines.len(), 2);
            assert!(
                (lines[1].start.x - 50.0).abs() < 1e-6 && (lines[1].start.y - 0.0).abs() < 1e-6
            );
            assert!((lines[1].end.x - 50.0).abs() < 1e-6 && (lines[1].end.y - 25.0).abs() < 1e-6);

            // Point cursor in -X direction from (50, 25) using feed_active_cmd
            app.tabs[0].last_cursor_world = glam::DVec3::new(0.0, 25.0, 0.0);
            let _ = app.feed_active_cmd("50");
            let lines: Vec<_> = app.tabs[0]
                .scene
                .document
                .entities()
                .filter_map(|e| match e {
                    codec::EntityType::Line(l) => Some(l.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(lines.len(), 3);
            assert!(
                (lines[2].start.x - 50.0).abs() < 1e-6 && (lines[2].start.y - 25.0).abs() < 1e-6
            );
            assert!(
                (lines[2].end.x - 0.0).abs() < 1e-6 && (lines[2].end.y - 25.0).abs() < 1e-6
            );
        });
    }
}

// ── Quad cursor menu tests ───────────────────────────────────────────────────
//
// Feed-style tests for the quad glue: right-click precedence, the pick
// dispatch, MRU recording and the hover-dwell step. Kept beside the context
// menu's own tests; the window itself is the parallel overlay's.

#[cfg(test)]
mod quad_tests {
    use super::*;
    use glam::DVec3;

    /// Run `f` on a thread with a production-sized stack — `update`'s
    /// recursion through the message router can exceed the test-thread
    /// default (same as the `tests` module above).
    fn with_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(f)
            .expect("spawn test thread")
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }

    /// Idle drawing, one 2D line, quad switched on with the default
    /// display options (hover/sel/right-click on, launch off).
    fn quadded_line_app() -> (OpenCADStudio, Handle) {
        let mut app = OpenCADStudio::new_for_test();
        app.automation_op(r#"{"op":"new"}"#);
        app.tabs[0].scene.selection.borrow_mut().view.vp_size = (800.0, 600.0);
        let _ = app.update(Message::ViewportMove(Point::new(200.0, 150.0)));
        app.quad = true;
        app.snapper.snap_enabled = false;
        app.snapper.grid_snap_on = false;
        app.ortho_mode = false;
        app.polar_mode = false;
        let _ = app.dispatch_command("LINE");
        let _ = app.feed_command(StepInput::Point(DVec3::new(0.0, 0.0, 0.0)));
        let _ = app.feed_command(StepInput::Point(DVec3::new(10.0, 0.0, 0.0)));
        let _ = app.update(Message::CommandEscape);
        let handle = app.tabs[0]
            .scene
            .document
            .entities()
            .find_map(|e| match e {
                codec::EntityType::Line(l) => Some(l.common.handle),
                _ => None,
            })
            .expect("the test line");
        (app, handle)
    }

    fn right_click(app: &mut OpenCADStudio) {
        let _ = app.update(Message::ViewportRightPress);
        let _ = app.update(Message::ViewportRightRelease);
    }

    fn quad_state(app: &OpenCADStudio) -> Option<QuadUi> {
        app.tabs[0].scene.selection.borrow().menu.quad.clone()
    }

    fn menu_open(app: &OpenCADStudio) -> bool {
        app.tabs[0]
            .scene
            .selection
            .borrow()
            .menu
            .open_at
            .is_some()
    }

    fn active(app: &OpenCADStudio) -> Option<&'static str> {
        app.tabs[0].active_cmd.as_ref().map(|c| c.name())
    }

    #[test]
    fn void_right_click_opens_no_selection_quad_when_enabled() {
        with_stack(|| {
            let (mut app, _) = quadded_line_app();
            // Nothing selected, nothing hovered: the No Selection quad
            // replaces the legacy shortcut menu.
            right_click(&mut app);
            let quad = quad_state(&app).expect("quad open on a void right-click");
            assert_eq!(quad.mode, QuadMode::NoSelection);
            assert!(!quad.expanded, "the quad opens collapsed");
            assert!(
                !menu_open(&app),
                "the legacy shortcut menu must not open beside the quad"
            );
            // Primary panel = the Recent panel, populated from the creation
            // groups without any MRU history.
            assert_eq!(app.quad_panels.first().map(|p| p.id.as_str()), Some("quad.recent"));
            assert!(!app.quad_panels[0].items.is_empty());
        });
    }

    #[test]
    fn selection_quad_replaces_the_shortcut_menu() {
        with_stack(|| {
            let (mut app, handle) = quadded_line_app();
            app.tabs[0].scene.select_entities(&[handle]);
            right_click(&mut app);
            let quad = quad_state(&app).expect("quad open on a selection right-click");
            assert_eq!(quad.mode, QuadMode::Selection);
            assert!(
                !menu_open(&app),
                "quad_on_select must replace the legacy shortcut menu"
            );
            assert_eq!(
                app.quad_panels.first().map(|p| p.id.as_str()),
                Some("quad.primary")
            );
            // Off: the legacy menu answers instead.
            let _ = app.update(Message::QuadOption(QuadOptionMsg::OnSelect(false)));
            right_click(&mut app);
            assert!(
                quad_state(&app).is_none(),
                "with quad_on_select off the selection takes the legacy path"
            );
            assert!(menu_open(&app));
        });
    }

    #[test]
    fn quad_pick_runs_the_plain_command_and_records_mru() {
        with_stack(|| {
            let (mut app, handle) = quadded_line_app();
            app.tabs[0].scene.select_entities(&[handle]);
            right_click(&mut app);
            let primary = app.quad_panels.first().cloned().expect("primary panel");
            // The curated tables lead with MOVE ("generic modify must lead").
            let expected = primary.items[0].command.clone();
            let _ = app.update(Message::Quad(QuadMsg::Pick {
                panel_id: primary.id.clone(),
                item_index: 0,
            }));
            // The quad closed and the command runs through the ordinary
            // dispatch path.
            assert!(quad_state(&app).is_none());
            assert!(app.quad_panels.is_empty());
            assert_eq!(active(&app), Some(expected.as_str()));
            // Plain quad launches remember the pick under the selection's
            // last type key (plugin actions would not record).
            assert_eq!(app.quad_mru.recent_for("Line", 1), vec![expected]);
        });
    }

    #[test]
    fn quad_launch_runs_the_default_without_any_menu() {
        with_stack(|| {
            let (mut app, handle) = quadded_line_app();
            app.quad_command_launch = true;
            app.tabs[0].scene.hover_highlight = Some(handle);
            right_click(&mut app);
            // Open-and-act: no quad, no context menu, the selection became
            // just the hovered entity and the default command is running.
            assert!(quad_state(&app).is_none());
            assert!(!menu_open(&app));
            assert!(app.tabs[0].scene.selected.contains(&handle));
            assert_eq!(app.tabs[0].scene.selected.len(), 1);
            assert_eq!(active(&app), Some("MOVE"));
            assert_eq!(app.quad_mru.recent_for("Line", 1), vec!["MOVE".to_string()]);
        });
    }

    #[test]
    fn right_click_over_entity_keeps_legacy_menu_without_launch() {
        with_stack(|| {
            let (mut app, handle) = quadded_line_app();
            // QUADCOMMANDLAUNCH off (the default): hovering an entity with
            // nothing selected does not offer the No Selection quad either —
            // the legacy menu answers.
            app.tabs[0].scene.hover_highlight = Some(handle);
            right_click(&mut app);
            assert!(quad_state(&app).is_none());
            assert!(menu_open(&app));
            assert!(active(&app).is_none());
        });
    }

    #[test]
    fn quad_off_right_click_is_pure_legacy() {
        with_stack(|| {
            let (mut app, _) = quadded_line_app();
            app.quad = false;
            right_click(&mut app);
            assert!(quad_state(&app).is_none());
            assert!(menu_open(&app), "master off: full legacy behaviour");
            // Toggle the feature back on through the router (the
            // Shift+F12 / status-bar path).
            let _ = app.update(Message::Quad(QuadMsg::Toggle));
            right_click(&mut app);
            assert!(quad_state(&app).is_some());
        });
    }

    #[test]
    fn enter_picks_the_primary_default_with_no_text_pending() {
        with_stack(|| {
            let (mut app, _) = quadded_line_app();
            right_click(&mut app); // No Selection quad, fresh MRU
            let first = app.quad_panels[0].items[0].command.clone();
            let _ = app.update(Message::Quad(QuadMsg::Pick {
                panel_id: "quad.recent".into(),
                item_index: 99, // out of range: ignored
            }));
            assert_eq!(active(&app), None, "out-of-range picks are ignored");
            // Enter with the command line empty picks the primary item.
            let _ = app.update(Message::Quad(QuadMsg::Pick {
                panel_id: "quad.recent".into(),
                item_index: 0,
            }));
            assert_eq!(active(&app), Some(first.as_str()));
        });
    }

    #[test]
    fn hover_dwell_opens_the_quad_then_drift_or_loss_closes_it() {
        with_stack(|| {
            let (mut app, handle) = quadded_line_app();
            // The settled rollover holds an entity and the cursor rests: the
            // first step arms, a step that already rested long enough opens.
            app.tabs[0].scene.hover_highlight = Some(handle);
            app.quad_hover_step(0, Point::new(100.0, 100.0), 0);
            assert!(app.quad_hover_since.is_some());
            assert!(quad_state(&app).is_none());
            app.quad_hover_step(0, Point::new(100.0, 100.0), 320);
            let quad = quad_state(&app).expect("the hover quad opened past the delay");
            assert_eq!(quad.mode, QuadMode::Hover);
            // On the way up and right to its tile it stays open, even once
            // the cursor leaves the object.
            app.tabs[0].scene.hover_highlight = None;
            app.quad_hover_step(0, Point::new(130.0, 80.0), 0);
            assert!(quad_state(&app).is_some());
            // Any other way, past 8px from the open anchor, closes it again.
            app.quad_hover_step(0, Point::new(80.0, 100.0), 0);
            assert!(quad_state(&app).is_none());
            assert!(app.quad_hover_since.is_none());
        });
    }

    #[test]
    fn hover_quad_suppressed_by_selection_and_by_inactive_features() {
        with_stack(|| {
            let (mut app, handle) = quadded_line_app();
            app.tabs[0].scene.hover_highlight = Some(handle);
            app.tabs[0].scene.select_entities(&[handle]);
            // Suppress-on-selection is the default: no hover quad.
            app.quad_hover_step(0, Point::new(5.0, 5.0), 0);
            app.quad_hover_step(0, Point::new(5.0, 5.0), 320);
            assert!(quad_state(&app).is_none());
            // Suppression off: the dwell counts and opens.
            let _ = app.update(Message::QuadOption(QuadOptionMsg::SuppressHoverWhenSelection(
                false,
            )));
            app.quad_hover_step(0, Point::new(5.0, 5.0), 0);
            app.quad_hover_step(0, Point::new(5.0, 5.0), 320);
            assert!(quad_state(&app).is_some());
            // Turning the quad off while it is open closes it.
            let _ = app.update(Message::Quad(QuadMsg::Toggle));
            assert!(quad_state(&app).is_none());
            assert!(app.quad_panels.is_empty());
            assert!(!app.quad, "the router flip persisted through the option");
        });
    }

    #[test]
    fn quad_options_clamp_and_toggle_like_their_sysvars() {
        with_stack(|| {
            let (mut app, _) = quadded_line_app();
            let _ = app.update(Message::QuadOption(QuadOptionMsg::Width(99)));
            assert_eq!(
                app.quad_width,
                crate::app::settings::clamp_quad_width(99)
            );
            let _ = app.update(Message::QuadOption(QuadOptionMsg::HoverDelay(9_999)));
            assert_eq!(app.quad_hover_delay_ms, 2_000);
            let _ = app.update(Message::QuadOption(QuadOptionMsg::Quad(false)));
            assert!(!app.quad);
        });
    }

    /// The hover quad's subentity test fixture: the hovered SOLID carries a
    /// read body under a fixed crosshair spot, so the resolution machinery
    /// (candidates, wire highlight, gate matrix) is exercised end-to-end —
    /// docs/quad-menu.md §3.5.1.
    fn quadded_solid_app() -> (OpenCADStudio, Handle, Point) {
        let mut app = OpenCADStudio::new_for_test();
        app.automation_op(r#"{"op":"new"}"#);
        app.tabs[0].scene.selection.borrow_mut().view.vp_size = (800.0, 600.0);
        app.quad = true;
        app.snapper.snap_enabled = false;
        app.snapper.grid_snap_on = false;
        app.polar_mode = false;
        app.ortho_mode = false;
        let solid = app.tabs[0].scene.add_entity(codec::EntityType::Solid3D(
            codec::entities::Solid3D::new(),
        ));
        // A read body under the fixed crosshair spot, so the hover
        // classification resolves: the box's top face *contains* the
        // cursor's world point exactly.
        let pos = Point::new(300.0, 300.0);
        let world = app.cursor_world_at(0, pos);
        let body = crate::scene::model::solid_model::box_solid(
            [world.x, world.y, world.z - 5.0],
            10.0,
            10.0,
            10.0,
        )
        .expect("box body");
        app.tabs[0].scene.solid_models.insert(solid, body);
        app.tabs[0].scene.hover_highlight = Some(solid);
        // The settled crosshair, so the re-classification reads the same
        // spot (the move pipeline keeps this fresh in production).
        app.tabs[0].last_cursor_screen = pos;
        // A real view scale so the pick aperture converts to world units.
        app.tabs[0].scene.set_render_pixel_scale(800.0, 600.0);
        (app, solid, pos)
    }

    /// `quad_acis_faces` gates the hover quad's SUBENTITY resolution now
    /// (docs/quad-menu.md §3.5.1): 0 = never, 1 = Shift+hover (default,
    /// read at open), 2 = plain hover — face-bearing types only (Solid3D
    /// et al.), and the hover quad only; the Selection quad is out of the
    /// game entirely. The kind-function panels ride `QuadTarget::Hovered::sub`
    /// as the primary panel (the old separate "Faces" tab is gone), and the
    /// wire overlay ("QUAD-SUBENTITY-HOVER") carries the highlight.
    #[test]
    fn hover_quad_resolves_subentities_per_setting_and_shift() {
        use crate::scene::pick::subentity::SubKind;
        with_stack(|| {
            let (mut app, solid, pos) = quadded_solid_app();
            // Defaults (mode 1 = Shift+hover, no Shift held): the solid-level quad.
            assert!(app.open_quad(QuadMode::Hover, pos));
            assert!(
                matches!(
                    app.quad_open_target,
                    Some(QuadTarget::Hovered { sub: None, .. })
                ),
                "the default gate resolves nothing while Shift is not held"
            );
            assert!(!app.quad_panels[0].items.iter().any(|i| i.command == "PRESSPULL"));
            assert!(app.tabs[0].scene.preview_wires.is_empty());

            // Holding Shift at (re-)open: the nearest subentity resolves
            // (the box's top face here), becomes the target's sub, the
            // funcs become the primary, the candidates stage for Tab and
            // the highlight wire lands.
            app.shift_down = true;
            assert!(app.open_quad(QuadMode::Hover, pos));
            let Some(QuadTarget::Hovered { sub: Some(resolved), .. }) = &app.quad_open_target else {
                panic!("a Shift+hover open resolves the hovered face");
            };
            assert_eq!(resolved.kind, SubKind::Face);
            assert!(app.quad_panels[0]
                .items
                .iter()
                .any(|i| i.command == "PRESSPULL"), "the face funcs are the primary");
            assert!(!app.tabs[0].scene.preview_wires.is_empty(),
                "the highlight wires carry the resolved face");
            assert!(
                !quad_state(&app)
                    .expect("open")
                    .sub_candidates
                    .is_empty(),
                "the resolved candidates stage for the Tab cycle"
            );

            // Mode 0 keeps everything solid-level even while Shift is held.
            let _ = app.update(Message::QuadOption(QuadOptionMsg::AcisFaces(0)));
            assert_eq!(app.quad_acis_faces, 0);
            assert!(app.open_quad(QuadMode::Hover, pos));
            assert!(matches!(
                app.quad_open_target,
                Some(QuadTarget::Hovered { sub: None, .. })
            ));
            assert!(app.tabs[0].scene.preview_wires.is_empty());

            // Mode 2 (plain hover) resolves without Shift; an out-of-range
            // value clamps back into the domain (2 = "always on").
            let _ = app.update(Message::QuadOption(QuadOptionMsg::AcisFaces(2)));
            assert_eq!(app.quad_acis_faces, 2);
            app.shift_down = false;
            assert!(app.open_quad(QuadMode::Hover, pos));
            assert!(matches!(
                app.quad_open_target,
                Some(QuadTarget::Hovered { sub: Some(_), .. })
            ));
            let _ = app.update(Message::QuadOption(QuadOptionMsg::AcisFaces(77)));
            assert_eq!(app.quad_acis_faces, 2, "an oversized mode clamps to 'always on'");

            // Selection-mode opens never resolve subentities, whatever the
            // setting and Shift state.
            app.quad_acis_faces = 2;
            app.shift_down = true;
            app.tabs[0].scene.select_entities(&[solid]);
            assert!(app.open_quad(QuadMode::Selection, pos));
            assert!(matches!(app.quad_open_target, Some(QuadTarget::Selection { .. })));
        });
    }

    #[test]
    fn quad_subentity_wires_clear_on_close() {
        with_stack(|| {
            let (mut app, _solid, pos) = quadded_solid_app();
            app.quad_acis_faces = 2;
            assert!(app.open_quad(QuadMode::Hover, pos));
            assert!(!app.tabs[0].scene.preview_wires.is_empty());
            let _ = app.update(Message::Quad(QuadMsg::Close));
            assert!(app.tabs[0].scene.preview_wires.is_empty(), "the wires clear");
            assert!(app.quad_panels.is_empty());
        });
    }

    /// The subentity workflow's immediate answer (docs/quad-menu.md
    /// §3.5.1): mode 1's Shift+hover on a face-bearing type opens the quad
    /// without waiting the configured dwell out — and the Shift flip while
    /// the quad is already open (or a dwell is counting) re-classifies in
    /// the same update.
    #[test]
    fn shift_held_hover_on_a_solid_opens_without_the_dwell() {
        with_stack(|| {
            let (mut app, _solid, pos) = quadded_solid_app();
            // Defaults (mode 1, no Shift): the dwell counts.
            app.quad_hover_step(0, pos, 0);
            assert!(quad_state(&app).is_none(), "no Shift: the dwell counts");
            assert!(app.quad_hover_since.is_some());
            // Pressing Shift re-runs the settled step from the flip: mode 1
            // opens without the dwell, the face resolves at once.
            app.shift_down = true;
            app.quad_hover_step(0, pos, 0);
            let quad = quad_state(&app).expect("Shift+hover opens without the dwell");
            assert_eq!(quad.mode, QuadMode::Hover);
            assert!(app.quad_panels[0].items.iter().any(|i| i.command == "PRESSPULL"));
            assert!(!app.tabs[0].scene.preview_wires.is_empty());
            assert_eq!(quad.anchor, pos, "opened at the resting cursor");
        });
    }

    #[test]
    fn shift_flip_reanswers_an_open_hover_quad_without_the_dwell() {
        use crate::scene::pick::subentity::SubKind;
        with_stack(|| {
            let (mut app, _solid, pos) = quadded_solid_app();
            // A solid-level hover quad is open (mode 1, no Shift, dwell-late).
            app.quad_hover_step(0, pos, 0);
            app.quad_hover_step(0, pos, 320);
            let quad = quad_state(&app).expect("opened after the dwell");
            assert_eq!(quad.anchor, pos);
            let anchor = quad.anchor;
            assert!(matches!(
                app.quad_open_target,
                Some(QuadTarget::Hovered { sub: None, .. })
            ));
            // The SetModifiers flip re-classifies in the same update: no
            // dwell, no re-anchor — the sub resolves, the funcs show, the
            // wire appears. The window state stays collapsed (nothing was
            // expanded for the flip to preserve).
            let _ = app.update(Message::SetModifiers {
                shift: true,
                ctrl: false,
                alt: false,
            });
            let Some(QuadTarget::Hovered { sub: Some(resolved), .. }) = &app.quad_open_target else {
                panic!("the flip resolved the face subentity");
            };
            assert_eq!(resolved.kind, SubKind::Face);
            assert!(app.quad_panels[0].items.iter().any(|i| i.command == "PRESSPULL"));
            assert!(!app.tabs[0].scene.preview_wires.is_empty());
            assert_eq!(quad_state(&app).expect("open").anchor, anchor,
                "the anchor survives the re-classification");

            // Releasing Shift folds the target back out of the ACIS set:
            // the solid-level quad (and the wire with it), collapsed.
            let _ = app.update(Message::SetModifiers {
                shift: false,
                ctrl: false,
                alt: false,
            });
            assert!(matches!(
                app.quad_open_target,
                Some(QuadTarget::Hovered { sub: None, .. })
            ));
            assert!(app.tabs[0].scene.preview_wires.is_empty());
            assert!(!quad_state(&app).expect("open").expanded, "the ACIS-leaving fold collapses");
        });
    }

    /// A flip while only the hover dwell counts re-runs the step at the
    /// resting cursor — mode 1's Shift+hover rule opens it without waiting
    /// the quad delay out.
    #[test]
    fn shift_flip_reanswers_the_counting_dwell_immediately_too() {
        with_stack(|| {
            let (mut app, _solid, pos) = quadded_solid_app();
            app.tabs[0].last_cursor_screen = pos;
            app.quad_hover_step(0, pos, 0); // arming only (mode 1, no Shift)
            assert!(quad_state(&app).is_none());
            let _ = app.update(Message::SetModifiers {
                shift: true,
                ctrl: false,
                alt: false,
            });
            assert!(quad_state(&app).is_some(), "the flip opened the quad");
            assert!(app.quad_panels[0].items.iter().any(|i| i.command == "PRESSPULL"));
        });
    }

    /// The keyboard Tab with a subentity quad open cycles the staged
    /// candidates — faces, edges and vertices as they rank, wrapping —
    /// each step swapping the primary panel (and the wire highlight) at
    /// once; without candidates the hover quad keeps its key fall-through.
    #[test]
    fn quad_tab_cycles_the_staged_subentities() {
        use crate::scene::pick::subentity::{SubKey, SubKind, SubTarget};
        with_stack(|| {
            let (mut app, _solid, pos) = quadded_solid_app();
            // Mode 1 + Shift first settled step: open + face resolved.
            app.shift_down = true;
            app.quad_hover_step(0, pos, 0);
            assert!(quad_state(&app).is_some());
            // Stub the cycle universe: face → edge → vertex (the stage-1
            // ranked order), the face as the live sub.
            let subtarget = |kind, idx| SubTarget {
                kind,
                key: SubKey { idx, gen: 0 },
                point: glam::DVec3::ZERO,
            };
            let (face, edge, vertex) = (
                subtarget(SubKind::Face, 1),
                subtarget(SubKind::Edge, 2),
                subtarget(SubKind::Vertex, 3),
            );
            {
                let mut sel = app.tabs[0].scene.selection.borrow_mut();
                let quad = sel.menu.quad.as_mut().expect("open");
                quad.sub_candidates = vec![face.clone(), edge.clone(), vertex.clone()];
                quad.sub_index = 0;
            }
            // The keyboard Tab (DynTabNext through the update router)
            // cycles subentity kind — off the face funcs row to the edge
            // ones — and re-draws the highlight at once.
            let _ = app.update(Message::DynTabNext);
            let Some(QuadTarget::Hovered { sub: Some(resolved), .. }) = &app.quad_open_target else {
                panic!("the cycle re-pointed the sub");
            };
            assert_eq!(resolved.kind, SubKind::Edge);
            assert!(app.quad_panels[0]
                .items
                .iter()
                .all(|i| ["FILLETEDGE", "CHAMFEREDGE"].contains(&i.command.as_str())),
                "the edge funcs are the primary after the turn to an edge"
            );
            assert!(!app.tabs[0].scene.preview_wires.is_empty());

            // Vertex: no functions yet — the solid-level panel returns
            // (and a vertex carries no highlight wire at all).
            let _ = app.update(Message::DynTabNext);
            let Some(QuadTarget::Hovered { sub: Some(resolved), .. }) = &app.quad_open_target else {
                panic!("the cycle re-pointed the sub");
            };
            assert_eq!(resolved.kind, SubKind::Vertex);
            assert!(app.quad_panels[0].items.iter().any(|i| i.command == "MOVE"),
                "the solid-level primary survives the vertex turn");
            assert!(app.tabs[0].scene.preview_wires.is_empty(),
                "a vertex carries no highlight wire");

            // Wraps forward (vertex → face), and backward from the current
            // one (face → vertex) — the wrap is modulo ALL candidates.
            let _ = app.update(Message::DynTabNext);
            assert!(matches!(
                &app.quad_open_target,
                Some(QuadTarget::Hovered { sub: Some(resolved), .. }) if resolved.kind == SubKind::Face
            ));
            let _ = app.update(Message::DynTabPrev);
            assert!(matches!(
                &app.quad_open_target,
                Some(QuadTarget::Hovered { sub: Some(resolved), .. }) if resolved.kind == SubKind::Vertex
            ));
        });
    }

    /// No staged candidates (a non-solid hover / the gate off): the
    /// keyboard Tab keeps the hover quad's fall-through law — it closes the
    /// quad and lets the key through, exactly like every other key.
    #[test]
    fn quad_tab_without_candidates_keeps_the_fall_through() {
        with_stack(|| {
            let (mut app, handle) = quadded_line_app();
            app.tabs[0].scene.hover_highlight = Some(handle);
            app.quad_hover_step(0, Point::new(100.0, 100.0), 0);
            app.quad_hover_step(0, Point::new(100.0, 100.0), 320);
            assert!(quad_state(&app).is_some());
            let _ = app.update(Message::DynTabNext);
            assert!(
                quad_state(&app).is_none(),
                "no candidates: the Tab closes the hover quad like before"
            );
        });
    }

    /// The active tab toggles back through the router: `Tab(0)` keeps the
    /// quad open and expanded on the primary panel (`active_tab` 0), the
    /// state the view's active-tab emission (`quad_tab`) resolves to.
    #[test]
    fn quad_tab_zero_returns_the_primary_and_stays_expanded() {
        with_stack(|| {
            let (mut app, handle) = quadded_line_app();
            // A Line overflows into group tabs at a narrow width.
            let _ = app.update(Message::QuadOption(QuadOptionMsg::Width(3)));
            app.tabs[0].scene.hover_highlight = Some(handle);
            app.quad_hover_step(0, Point::new(100.0, 100.0), 0);
            app.quad_hover_step(0, Point::new(100.0, 100.0), 320);
            let quad = quad_state(&app).expect("the hover quad opened");
            assert_eq!(quad.mode, QuadMode::Hover);
            assert!(app.quad_panels.len() > 1, "the test needs at least one tab");
            let _ = app.update(Message::Quad(QuadMsg::Tab(1)));
            assert_eq!(quad_state(&app).expect("open").active_tab, 1);
            // The toggle-back emissions (`Tab(0)`) resolve to the primary,
            // still expanded — no re-collapse, no close.
            let _ = app.update(Message::Quad(QuadMsg::Tab(0)));
            let quad = quad_state(&app).expect("still open");
            assert_eq!(quad.active_tab, 0);
            assert!(quad.expanded);
            assert_eq!(app.quad_panels[0].id, "quad.primary");
        });
    }

    /// A quad pick on an unselected hovered target replaces the selection
    /// with that target before the command runs (docs/quad-menu.md §3.4),
    /// recording under the target's type key.
    #[test]
    fn hover_quad_pick_replaces_the_selection_with_the_hovered() {
        with_stack(|| {
            let (mut app, handle) = quadded_line_app();
            let _ = app.run_command_line("LINE 20,0 30,0");
            let other = app.tabs[0]
                .scene
                .document
                .entities()
                .find_map(|e| match e {
                    codec::EntityType::Line(l) if l.common.handle != handle => {
                        Some(l.common.handle)
                    }
                    _ => None,
                })
                .expect("the second test line");
            // A lingering selection is replaced, and the hover quad must be
            // allowed despite it.
            app.tabs[0].scene.select_entities(&[other]);
            let _ = app.update(Message::QuadOption(QuadOptionMsg::SuppressHoverWhenSelection(
                false,
            )));
            app.tabs[0].scene.hover_highlight = Some(handle);
            app.quad_hover_step(0, Point::new(100.0, 100.0), 0);
            app.quad_hover_step(0, Point::new(100.0, 100.0), 320);
            assert!(quad_state(&app).is_some(), "the hover quad opened");
            let primary = app.quad_panels.first().cloned().expect("primary panel");
            let expected = primary.items[0].command.clone();
            let _ = app.update(Message::Quad(QuadMsg::Pick {
                panel_id: primary.id.clone(),
                item_index: 0,
            }));
            assert_eq!(active(&app), Some(expected.as_str()));
            assert!(
                app.tabs[0].scene.selected.contains(&handle) && app.tabs[0].scene.selected.len() == 1,
                "the pick aligned the selection to the hovered entity"
            );
            assert_eq!(app.quad_mru.recent_for("Line", 1), vec![expected]);
        });
    }

    /// The Close message is the canonical teardown: the window state, the
    /// built panels, the open-target classification and the hover dwell all
    /// drop together.
    #[test]
    fn quad_close_message_clears_the_app_state() {
        with_stack(|| {
            let (mut app, _) = quadded_line_app();
            right_click(&mut app);
            assert!(quad_state(&app).is_some());
            assert!(!app.quad_panels.is_empty());
            let _ = app.update(Message::Quad(QuadMsg::Close));
            assert!(quad_state(&app).is_none());
            assert!(app.quad_panels.is_empty());
            assert!(app.quad_hover_since.is_none());
        });
    }

    /// Tab cycles modulo ALL panels: it reaches the group tab, wraps through
    /// the primary (index 0) and lands on the last panel cycling back.
    #[test]
    fn quad_tab_cycle_wraps_through_the_primary_and_reaches_the_last() {
        with_stack(|| {
            let (mut app, handle) = quadded_line_app();
            app.tabs[0].scene.select_entities(&[handle]);
            right_click(&mut app); // Selection quad: primary + group tab(s)
            let count = app.quad_panels.len();
            assert!(count >= 2, "a wide panel set to cycle ({count})");
            // Forward: 0 → 1 → … → count-1 → 0, the wrap passing through
            // the primary (index 0 — the old len-1 modulus never reached 0
            // after the first wrap and always skipped the last panel).
            for step in 1..count {
                assert!(app.quad_cycle_tabs(1));
                let quad = quad_state(&app).expect("quad stays open");
                assert!(quad.expanded, "cycling expands");
                assert_eq!(quad.active_tab, step, "the tab panel is reachable");
            }
            assert!(app.quad_cycle_tabs(1));
            assert_eq!(
                quad_state(&app).unwrap().active_tab,
                0,
                "the wrap goes through the primary"
            );
            assert!(app.quad_cycle_tabs(-1));
            assert_eq!(
                quad_state(&app).unwrap().active_tab,
                count - 1,
                "the last panel is reachable cycling back"
            );
            // A quad with only the primary panel has no tabs: no cycle.
            let (mut app, _) = quadded_line_app();
            right_click(&mut app); // No Selection quad may hold a single panel
            if app.quad_panels.len() == 1 {
                assert!(!app.quad_cycle_tabs(1));
            }
        });
    }

    /// The launch default follows the quad model's primary: the type's MRU
    /// head when one exists, else the curated first.
    #[test]
    fn quad_launch_resolves_the_mru_head_as_the_default() {
        with_stack(|| {
            let (mut app, handle) = quadded_line_app();
            app.quad_command_launch = true;
            app.quad_mru.record_for("Line", "BREAK");
            app.tabs[0].scene.hover_highlight = Some(handle);
            right_click(&mut app);
            assert!(
                quad_state(&app).is_none() && !menu_open(&app),
                "still open-and-act"
            );
            assert_eq!(active(&app), Some("BREAK"), "the MRU head leads");
            assert_eq!(app.quad_mru.recent_for("Line", 1), vec!["BREAK".to_string()]);
        });
    }

    /// A held right-click over an entity does not launch: like the void's,
    /// the hold opens the No Selection quad at the cursor; a quick click
    /// still launches (the same release the press duration guards).
    #[test]
    fn quad_launch_hold_opens_the_no_selection_quad() {
        with_stack(|| {
            let (mut app, handle) = quadded_line_app();
            app.quad_command_launch = true;
            app.tabs[0].scene.hover_highlight = Some(handle);
            let _ = app.update(Message::ViewportRightPress);
            {
                let mut sel = app.tabs[0].scene.selection.borrow_mut();
                sel.input.right_press_time =
                    iced::time::Instant::now().checked_sub(std::time::Duration::from_millis(400));
            }
            let _ = app.update(Message::ViewportRightRelease);
            let quad = quad_state(&app).expect("the hold opens the No Selection quad");
            assert_eq!(quad.mode, QuadMode::NoSelection);
            assert_eq!(active(&app), None, "no launch happened");
            assert!(
                app.quad_mru.recent_for("Line", 1).is_empty(),
                "a hold records nothing"
            );
        });
    }

    /// A digit typed with pending command-line text is not swallowed by a
    /// digit pick: the quad closes and the key reaches the command line.
    #[test]
    fn quad_digit_with_pending_text_falls_through_to_the_command_line() {
        with_stack(|| {
            let (mut app, _) = quadded_line_app();
            right_click(&mut app); // No Selection quad
            assert!(quad_state(&app).is_some());
            app.command_line.input = "3".into();
            let _ = app.update(Message::CommandAppendChar("5".into()));
            assert!(
                quad_state(&app).is_none(),
                "pending text closes the quad"
            );
            assert_eq!(app.command_line.input, "35", "the digit reached the line");
            assert_eq!(active(&app), None);
        });
    }

    /// A tab switch does not drag the quad along: its window state is
    /// per-tab while the panels / open target are app-global (docs/
    /// quad-menu.md §3.1), so the switch runs the canonical teardown on the
    /// outgoing side — a later stale pick must not re-target the outgoing
    /// drawing's handles.
    #[test]
    fn tab_switch_closes_the_quad() {
        with_stack(|| {
            let (mut app, _) = quadded_line_app();
            right_click(&mut app); // No Selection quad on tab 0
            assert!(quad_state(&app).is_some());
            assert!(!app.quad_panels.is_empty());
            let _ = app.update(Message::TabNew);
            // TabNew is the same disruptive transition (fresh drawing):
            // the quad is dropped with the switch to it, so no dormant
            // window state can outlive the app-global teardown.
            assert!(quad_state(&app).is_none(), "TabNew drops the quad");
            let _ = app.update(Message::TabSwitch(0));
            // The canonical teardown ran on the switch; a stale pick finds
            // neither window state nor panels and runs nothing.
            let _ = app.update(Message::Quad(QuadMsg::Pick {
                panel_id: "quad.recent".into(),
                item_index: 0,
            }));
            assert!(quad_state(&app).is_none());
            assert!(app.quad_panels.is_empty());
            assert_eq!(active(&app), None, "the stale pick ran nothing");
        });
    }
}
