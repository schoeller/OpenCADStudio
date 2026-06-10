//! Shared CRUD layer for every style manager (text / dimension / table /
//! multileader / multiline).
//!
//! The five managers all expose the same list operations — New, Copy, Delete,
//! Rename, Set-Current — over a named collection of styles. Only the *property
//! editor* and the *storage backend* differ, so those are the only parts kept
//! per-manager:
//!
//! * **Table-backed** (text, dim): live in `Table<T>`, keyed by upper-cased
//!   name. Renaming must re-key the entry and rewrite name-based entity
//!   references (TEXT/MTEXT `style`, DIMENSION `style_name`).
//! * **Object-backed** (table, multileader, multiline): live in
//!   `document.objects`, keyed by handle. Renaming only mutates the `name`
//!   field; entities reference these by handle, so nothing else moves.
//!
//! Centralising the flow here is what fixes the bug class that kept recurring
//! when each manager was hand-copied: a dead New, a missing ribbon refresh, a
//! style added without a handle (dropped on DWG save, issue #67).

use super::OpenCADStudio;
use acadrust::objects::{MLineStyle, MultiLeaderStyle, ObjectType, TableStyle};
use acadrust::tables::{DimStyle, TextStyle};
use acadrust::types::Handle;

/// Which style manager an operation targets. Carried by the shared rename
/// messages so one handler can dispatch to the right storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleKind {
    Text,
    Dim,
    Table,
    MLeader,
    MLine,
}

impl StyleKind {
    /// True when this style feeds the ribbon's quick-set dropdown.
    fn in_ribbon(self) -> bool {
        !matches!(self, StyleKind::MLine)
    }
}

impl OpenCADStudio {
    // ── Queries ────────────────────────────────────────────────────────────

    /// All style names for `kind`, in display order (object-backed styles are
    /// sorted by name so the `HashMap` backing them renders stably).
    pub(super) fn style_names(&self, kind: StyleKind) -> Vec<String> {
        let doc = &self.tabs[self.active_tab].scene.document;
        let mut from_objects = |pick: fn(&ObjectType) -> Option<&str>| -> Vec<String> {
            let mut v: Vec<String> = doc
                .objects
                .values()
                .filter_map(pick)
                .map(str::to_string)
                .collect();
            v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
            v
        };
        match kind {
            StyleKind::Text => doc.text_styles.iter().map(|s| s.name.clone()).collect(),
            StyleKind::Dim => doc.dim_styles.iter().map(|s| s.name.clone()).collect(),
            StyleKind::Table => from_objects(|o| match o {
                ObjectType::TableStyle(s) => Some(s.name.as_str()),
                _ => None,
            }),
            StyleKind::MLeader => from_objects(|o| match o {
                ObjectType::MultiLeaderStyle(s) => Some(s.name.as_str()),
                _ => None,
            }),
            StyleKind::MLine => from_objects(|o| match o {
                ObjectType::MLineStyle(s) => Some(s.name.as_str()),
                _ => None,
            }),
        }
    }

    pub(super) fn style_selected(&self, kind: StyleKind) -> String {
        match kind {
            StyleKind::Text => self.textstyle_selected.clone(),
            StyleKind::Dim => self.dimstyle_selected.clone(),
            StyleKind::Table => self.tablestyle_selected.clone(),
            StyleKind::MLeader => self.mleaderstyle_selected.clone(),
            StyleKind::MLine => self.mlstyle_selected.clone(),
        }
    }

    fn set_style_selected(&mut self, kind: StyleKind, name: String) {
        match kind {
            StyleKind::Text => self.textstyle_selected = name,
            StyleKind::Dim => self.dimstyle_selected = name,
            StyleKind::Table => self.tablestyle_selected = name,
            StyleKind::MLeader => self.mleaderstyle_selected = name,
            StyleKind::MLine => self.mlstyle_selected = name,
        }
    }

    fn style_exists(&self, kind: StyleKind, name: &str) -> bool {
        self.style_names(kind)
            .iter()
            .any(|n| n.eq_ignore_ascii_case(name))
    }

    /// First free `Style{n}` name for a fresh style.
    fn unique_new_name(&self, kind: StyleKind) -> String {
        (1u32..)
            .map(|n| format!("Style{n}"))
            .find(|c| !self.style_exists(kind, c))
            .unwrap()
    }

    /// First free `{base} ({n})` name for a copy / disambiguated entry.
    fn unique_suffixed_name(&self, kind: StyleKind, base: &str) -> String {
        (1u32..)
            .map(|n| format!("{base} ({n})"))
            .find(|c| !self.style_exists(kind, c))
            .unwrap()
    }

    // ── Per-manager glue (the only kind-specific list code) ────────────────

    /// Reload the property-editor buffers for the kinds that have them.
    fn load_style_bufs(&mut self, kind: StyleKind) {
        let i = self.active_tab;
        match kind {
            StyleKind::Text => self.load_textstyle_bufs(i),
            StyleKind::Dim => self.load_dimstyle_bufs(i),
            StyleKind::MLeader => self.load_mleaderstyle_bufs(i),
            StyleKind::Table | StyleKind::MLine => {}
        }
    }

    /// Refresh anything that mirrors the style list / current style after a
    /// mutation (ribbon dropdowns, geometry that depends on the style).
    fn after_style_change(&mut self, kind: StyleKind) {
        if kind.in_ribbon() {
            self.sync_ribbon_styles();
        }
    }

    fn insert_default_style(&mut self, kind: StyleKind, name: &str, handle: Handle) {
        let doc = &mut self.tabs[self.active_tab].scene.document;
        match kind {
            StyleKind::Text => {
                let mut s = TextStyle::new(name);
                s.handle = handle;
                let _ = doc.text_styles.add(s);
            }
            StyleKind::Dim => {
                let mut s = DimStyle::new(name);
                s.handle = handle;
                let _ = doc.dim_styles.add(s);
            }
            StyleKind::Table => {
                let mut s = TableStyle::standard();
                s.name = name.to_string();
                s.handle = handle;
                doc.objects.insert(handle, ObjectType::TableStyle(s));
            }
            StyleKind::MLeader => {
                let mut s = MultiLeaderStyle::new(name);
                s.handle = handle;
                doc.objects.insert(handle, ObjectType::MultiLeaderStyle(s));
            }
            StyleKind::MLine => {
                let mut s = MLineStyle::standard();
                s.name = name.to_string();
                s.handle = handle;
                doc.objects.insert(handle, ObjectType::MLineStyle(s));
            }
        }
    }

    /// Clone the style named `src` under `name` with a fresh `handle`.
    /// Returns false if `src` no longer exists.
    fn clone_style_as(&mut self, kind: StyleKind, src: &str, name: &str, handle: Handle) -> bool {
        let doc = &mut self.tabs[self.active_tab].scene.document;
        match kind {
            StyleKind::Text => {
                if let Some(mut s) = doc.text_styles.get(src).cloned() {
                    s.name = name.to_string();
                    s.handle = handle;
                    let _ = doc.text_styles.add(s);
                    return true;
                }
            }
            StyleKind::Dim => {
                if let Some(mut s) = doc.dim_styles.get(src).cloned() {
                    s.name = name.to_string();
                    s.handle = handle;
                    let _ = doc.dim_styles.add(s);
                    return true;
                }
            }
            StyleKind::Table => {
                if let Some(mut s) = find_object_style(doc, src, |o| match o {
                    ObjectType::TableStyle(s) => Some((s.name.as_str(), s.clone())),
                    _ => None,
                }) {
                    s.name = name.to_string();
                    s.handle = handle;
                    doc.objects.insert(handle, ObjectType::TableStyle(s));
                    return true;
                }
            }
            StyleKind::MLeader => {
                if let Some(mut s) = find_object_style(doc, src, |o| match o {
                    ObjectType::MultiLeaderStyle(s) => Some((s.name.as_str(), s.clone())),
                    _ => None,
                }) {
                    s.name = name.to_string();
                    s.handle = handle;
                    doc.objects.insert(handle, ObjectType::MultiLeaderStyle(s));
                    return true;
                }
            }
            StyleKind::MLine => {
                if let Some(mut s) = find_object_style(doc, src, |o| match o {
                    ObjectType::MLineStyle(s) => Some((s.name.as_str(), s.clone())),
                    _ => None,
                }) {
                    s.name = name.to_string();
                    s.handle = handle;
                    doc.objects.insert(handle, ObjectType::MLineStyle(s));
                    return true;
                }
            }
        }
        false
    }

    fn remove_style_storage(&mut self, kind: StyleKind, name: &str) -> bool {
        let doc = &mut self.tabs[self.active_tab].scene.document;
        match kind {
            StyleKind::Text => doc.text_styles.remove(name).is_some(),
            StyleKind::Dim => doc.dim_styles.remove(name).is_some(),
            StyleKind::Table | StyleKind::MLeader | StyleKind::MLine => {
                let kind2 = kind;
                if let Some(h) = object_handle(doc, name, kind2) {
                    doc.objects.remove(&h).is_some()
                } else {
                    false
                }
            }
        }
    }

    /// Rename `old`→`new` in the backing store, re-keying table entries and
    /// rewriting name-based references + current-style pointers.
    fn rename_style_storage(&mut self, kind: StyleKind, old: &str, new: &str) {
        let i = self.active_tab;
        match kind {
            StyleKind::Text => {
                let doc = &mut self.tabs[i].scene.document;
                if let Some(mut s) = doc.text_styles.get(old).cloned() {
                    s.name = new.to_string();
                    if !s.handle.is_valid() {
                        s.handle = doc.allocate_handle();
                    }
                    let _ = doc.text_styles.add(s);
                }
                doc.text_styles.remove(old);
                if doc.header.current_text_style_name.eq_ignore_ascii_case(old) {
                    doc.header.current_text_style_name = new.to_string();
                }
                for e in doc.entities_mut() {
                    match e {
                        acadrust::entities::EntityType::Text(t)
                            if t.style.eq_ignore_ascii_case(old) =>
                        {
                            t.style = new.to_string();
                        }
                        acadrust::entities::EntityType::MText(t)
                            if t.style.eq_ignore_ascii_case(old) =>
                        {
                            t.style = new.to_string();
                        }
                        _ => {}
                    }
                }
            }
            StyleKind::Dim => {
                let doc = &mut self.tabs[i].scene.document;
                if let Some(mut s) = doc.dim_styles.get(old).cloned() {
                    s.name = new.to_string();
                    if !s.handle.is_valid() {
                        s.handle = doc.allocate_handle();
                    }
                    let _ = doc.dim_styles.add(s);
                }
                doc.dim_styles.remove(old);
                if doc.header.current_dimstyle_name.eq_ignore_ascii_case(old) {
                    doc.header.current_dimstyle_name = new.to_string();
                }
                for e in doc.entities_mut() {
                    if let acadrust::entities::EntityType::Dimension(d) = e {
                        if d.base().style_name.eq_ignore_ascii_case(old) {
                            d.base_mut().style_name = new.to_string();
                        }
                    }
                }
            }
            StyleKind::Table => {
                let doc = &mut self.tabs[i].scene.document;
                if let Some(h) = object_handle(doc, old, kind) {
                    if let Some(ObjectType::TableStyle(s)) = doc.objects.get_mut(&h) {
                        s.name = new.to_string();
                    }
                }
                if self.ribbon.active_table_style.eq_ignore_ascii_case(old) {
                    self.ribbon.active_table_style = new.to_string();
                }
            }
            StyleKind::MLeader => {
                let doc = &mut self.tabs[i].scene.document;
                if let Some(h) = object_handle(doc, old, kind) {
                    if let Some(ObjectType::MultiLeaderStyle(s)) = doc.objects.get_mut(&h) {
                        s.name = new.to_string();
                    }
                }
                if self.tabs[i].active_mleader_style.eq_ignore_ascii_case(old) {
                    self.tabs[i].active_mleader_style = new.to_string();
                }
                if self.ribbon.active_mleader_style.eq_ignore_ascii_case(old) {
                    self.ribbon.active_mleader_style = new.to_string();
                }
            }
            StyleKind::MLine => {
                let doc = &mut self.tabs[i].scene.document;
                if let Some(h) = object_handle(doc, old, kind) {
                    if let Some(ObjectType::MLineStyle(s)) = doc.objects.get_mut(&h) {
                        s.name = new.to_string();
                    }
                }
                if doc.header.multiline_style.eq_ignore_ascii_case(old) {
                    doc.header.multiline_style = new.to_string();
                }
            }
        }
    }

    // ── Public operations (called by the message handlers) ─────────────────

    // Note: the structural ops below mutate the document live (so the dialog
    // shows a preview) but do NOT mark the tab dirty, push undo, or rebuild the
    // drawing. Those side effects are deferred to `style_stage_commit` (Apply);
    // closing the window without Apply calls `style_stage_discard`, which
    // restores the snapshot taken when the manager opened.

    pub(super) fn style_new(&mut self, kind: StyleKind) {
        let i = self.active_tab;
        let name = self.unique_new_name(kind);
        let h = self.tabs[i].scene.document.allocate_handle();
        self.insert_default_style(kind, &name, h);
        self.set_style_selected(kind, name.clone());
        self.load_style_bufs(kind);
        self.after_style_change(kind);
        self.command_line
            .push_output(&format!("Style '{name}' created."));
    }

    pub(super) fn style_copy(&mut self, kind: StyleKind) {
        let i = self.active_tab;
        let src = self.style_selected(kind);
        let name = self.unique_suffixed_name(kind, &src);
        let h = self.tabs[i].scene.document.allocate_handle();
        if !self.clone_style_as(kind, &src, &name, h) {
            return;
        }
        self.set_style_selected(kind, name.clone());
        self.load_style_bufs(kind);
        self.after_style_change(kind);
        self.command_line
            .push_output(&format!("Style '{name}' created."));
    }

    pub(super) fn style_delete(&mut self, kind: StyleKind) {
        let name = self.style_selected(kind);
        if name.eq_ignore_ascii_case("Standard") {
            self.command_line
                .push_error("Cannot delete the Standard style.");
            return;
        }
        if !self.remove_style_storage(kind, &name) {
            return;
        }
        let first = self
            .style_names(kind)
            .into_iter()
            .next()
            .unwrap_or_else(|| "Standard".to_string());
        self.set_style_selected(kind, first);
        self.load_style_bufs(kind);
        self.after_style_change(kind);
        self.command_line
            .push_output(&format!("Style '{name}' deleted."));
    }

    /// Begin inline rename of the double-clicked style.
    pub(super) fn style_rename_start(&mut self, kind: StyleKind, name: String) {
        self.set_style_selected(kind, name.clone());
        self.load_style_bufs(kind);
        self.style_rename_buf = name.clone();
        self.style_rename = Some(name);
    }

    /// Commit the inline rename. No-op (with feedback) on empty / unchanged /
    /// colliding names, and the Standard style cannot be renamed.
    pub(super) fn style_rename_commit(&mut self, kind: StyleKind) {
        let Some(old) = self.style_rename.take() else {
            return;
        };
        let new = self.style_rename_buf.trim().to_string();
        self.style_rename_buf.clear();
        if new.is_empty() || new.eq_ignore_ascii_case(&old) {
            return;
        }
        if old.eq_ignore_ascii_case("Standard") {
            self.command_line
                .push_error("Cannot rename the Standard style.");
            return;
        }
        if self.style_exists(kind, &new) {
            self.command_line
                .push_error(&format!("Style '{new}' already exists."));
            return;
        }
        self.rename_style_storage(kind, &old, &new);
        if self.style_selected(kind).eq_ignore_ascii_case(&old) {
            self.set_style_selected(kind, new.clone());
        }
        self.load_style_bufs(kind);
        self.after_style_change(kind);
        self.command_line
            .push_output(&format!("Renamed '{old}' → '{new}'."));
    }

    pub(super) fn style_rename_cancel(&mut self) {
        self.style_rename = None;
        self.style_rename_buf.clear();
    }

    // ── Staging: nothing persists until Apply ──────────────────────────────
    //
    // A style manager is a transaction. When it opens we snapshot the style
    // tables / objects / current pointers; every New / Copy / Delete / Rename /
    // Set Current / property edit mutates the live document for an in-dialog
    // preview, but the tab stays clean and the drawing is not rebuilt. Apply
    // commits (marks dirty, pushes one undo entry, rebuilds); closing the
    // window without Apply discards by restoring the snapshot.

    /// Snapshot the style-related document state into a transferable record.
    fn capture_style_state(&self) -> StyleStateSnapshot {
        let i = self.active_tab;
        let doc = &self.tabs[i].scene.document;
        let style_objects = doc
            .objects
            .iter()
            .filter(|(_, o)| {
                matches!(
                    o,
                    ObjectType::TableStyle(_)
                        | ObjectType::MLineStyle(_)
                        | ObjectType::MultiLeaderStyle(_)
                )
            })
            .map(|(&h, o)| (h, o.clone()))
            .collect();
        StyleStateSnapshot {
            text_styles: doc.text_styles.clone(),
            dim_styles: doc.dim_styles.clone(),
            style_objects,
            current_text: doc.header.current_text_style_name.clone(),
            current_dim: doc.header.current_dimstyle_name.clone(),
            multiline_style: doc.header.multiline_style.clone(),
            active_table: self.ribbon.active_table_style.clone(),
            active_mleader: self.ribbon.active_mleader_style.clone(),
            tab_active_mleader: self.tabs[i].active_mleader_style.clone(),
        }
    }

    /// Overwrite the live style state with a snapshot (used by commit's undo
    /// dance and by discard).
    fn restore_style_state(&mut self, snap: &StyleStateSnapshot) {
        let i = self.active_tab;
        let doc = &mut self.tabs[i].scene.document;
        doc.text_styles = snap.text_styles.clone();
        doc.dim_styles = snap.dim_styles.clone();
        doc.objects.retain(|_, o| {
            !matches!(
                o,
                ObjectType::TableStyle(_)
                    | ObjectType::MLineStyle(_)
                    | ObjectType::MultiLeaderStyle(_)
            )
        });
        for (h, o) in &snap.style_objects {
            doc.objects.insert(*h, o.clone());
        }
        doc.header.current_text_style_name = snap.current_text.clone();
        doc.header.current_dimstyle_name = snap.current_dim.clone();
        doc.header.multiline_style = snap.multiline_style.clone();
        self.ribbon.active_table_style = snap.active_table.clone();
        self.ribbon.active_mleader_style = snap.active_mleader.clone();
        self.tabs[i].active_mleader_style = snap.tab_active_mleader.clone();
    }

    /// Begin a staging transaction for a freshly-opened style manager.
    pub(super) fn style_stage_begin(&mut self) {
        let dirty_at_open = self.tabs[self.active_tab].dirty;
        let baseline = self.capture_style_state();
        self.style_stage = Some(StyleStage {
            dirty_at_open,
            baseline,
        });
    }

    /// Commit the staged changes (Apply): make them permanent with a single
    /// undo entry, mark the tab dirty, and rebuild the drawing.
    pub(super) fn style_stage_commit(&mut self) {
        let i = self.active_tab;
        let Some(stage) = self.style_stage.take() else {
            // No active transaction — still refresh the drawing so a bare Apply
            // is harmless.
            self.tabs[i].dirty = true;
            self.tabs[i].scene.bump_geometry();
            self.sync_ribbon_styles();
            return;
        };
        // Capture the edited state, rewind to the baseline so the undo entry
        // restores the pre-edit document, then re-apply the edits on top.
        let edited = self.capture_style_state();
        self.restore_style_state(&stage.baseline);
        self.push_undo_snapshot(i, "STYLE");
        self.restore_style_state(&edited);
        self.tabs[i].dirty = true;
        self.tabs[i].scene.bump_geometry();
        self.sync_ribbon_styles();
        // Re-baseline so further edits in the still-open window stage afresh.
        self.style_stage = Some(StyleStage {
            dirty_at_open: true,
            baseline: edited,
        });
    }

    /// Discard staged changes (window closed without Apply): restore the
    /// snapshot taken when the manager opened.
    pub(super) fn style_stage_discard(&mut self) {
        let Some(stage) = self.style_stage.take() else {
            return;
        };
        self.restore_style_state(&stage.baseline);
        self.tabs[self.active_tab].dirty = stage.dirty_at_open;
        self.sync_ribbon_styles();
    }
}

/// Snapshot of every document field a style manager can touch.
pub(super) struct StyleStateSnapshot {
    text_styles: acadrust::tables::Table<TextStyle>,
    dim_styles: acadrust::tables::Table<DimStyle>,
    style_objects: Vec<(Handle, ObjectType)>,
    current_text: String,
    current_dim: String,
    multiline_style: String,
    active_table: String,
    active_mleader: String,
    tab_active_mleader: String,
}

/// An in-progress style-manager transaction.
pub(super) struct StyleStage {
    dirty_at_open: bool,
    baseline: StyleStateSnapshot,
}

// ── Object-store helpers ───────────────────────────────────────────────────

/// Find the object-backed style named `name` and return a clone. `pick` maps a
/// matching variant to `(its name, a clone of the inner style)`.
fn find_object_style<T>(
    doc: &acadrust::CadDocument,
    name: &str,
    pick: impl Fn(&ObjectType) -> Option<(&str, T)>,
) -> Option<T> {
    doc.objects.values().find_map(|o| {
        let (n, val) = pick(o)?;
        n.eq_ignore_ascii_case(name).then_some(val)
    })
}

fn object_handle(doc: &acadrust::CadDocument, name: &str, kind: StyleKind) -> Option<Handle> {
    doc.objects.iter().find_map(|(&h, o)| {
        let matches = match (kind, o) {
            (StyleKind::Table, ObjectType::TableStyle(s)) => s.name.eq_ignore_ascii_case(name),
            (StyleKind::MLeader, ObjectType::MultiLeaderStyle(s)) => {
                s.name.eq_ignore_ascii_case(name)
            }
            (StyleKind::MLine, ObjectType::MLineStyle(s)) => s.name.eq_ignore_ascii_case(name),
            _ => false,
        };
        matches.then_some(h)
    })
}
