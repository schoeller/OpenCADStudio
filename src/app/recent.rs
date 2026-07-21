//! Recent-files list backing the Start page's Recent Documents panel. The list
//! itself lives in the consolidated app config (`settings.json`, the "recent"
//! section); this module just mutates the in-memory list and persists via
//! `save_config`.

use super::OpenCADStudio;
use std::path::{Path, PathBuf};

/// Bounds and default for how many recent files are kept.
pub(super) const RECENT_MIN: usize = 5;
pub(super) const RECENT_MAX: usize = 100;
pub(super) const RECENT_DEFAULT: usize = 20;

impl OpenCADStudio {
    /// Record a freshly opened file at the top of the recents list. Returns
    /// the background task that decodes its thumbnail.
    pub(super) fn push_recent(&mut self, path: PathBuf) -> iced::Task<crate::app::Message> {
        self.recent_files.retain(|r| r != &path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(self.recent_limit);
        self.save_config();
        self.refresh_recent_thumbs()
    }

    /// Decode any not-yet-cached DWG preview thumbnails for the current
    /// recents on a background thread, delivering them via
    /// [`Message::RecentThumbsLoaded`]. The synchronous version of this ran on
    /// the boot path and stalled the first frame for as long as it took to
    /// parse every recent DWG's preview — the Start page appeared seconds
    /// late. Cached per path (a `None` result is cached too); safe to call
    /// repeatedly.
    pub(super) fn refresh_recent_thumbs(&mut self) -> iced::Task<crate::app::Message> {
        let missing: Vec<std::path::PathBuf> = self
            .recent_files
            .iter()
            .filter(|p| !self.recent_thumbs.contains_key(*p))
            .cloned()
            .collect();
        if missing.is_empty() {
            return iced::Task::none();
        }
        // The web build has no spawnable threads and no filesystem previews —
        // recents there simply show without thumbnails.
        #[cfg(target_arch = "wasm32")]
        {
            return iced::Task::none();
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = iced::futures::channel::oneshot::channel();
            std::thread::spawn(move || {
                let thumbs: Vec<_> = missing
                    .into_iter()
                    .map(|p| {
                        let h = crate::io::thumbnail::read_handle(&p);
                        (p, h)
                    })
                    .collect();
                let _ = tx.send(thumbs);
            });
            iced::Task::perform(
                async move { rx.await.unwrap_or_default() },
                crate::app::Message::RecentThumbsLoaded,
            )
        }
    }

    /// Drop a path from the recents list (manual removal from the Start page).
    pub(super) fn remove_recent(&mut self, path: &Path) {
        self.recent_files.retain(|r| r.as_path() != path);
        self.save_config();
    }

    /// Set how many recent files are kept, trim the current list to fit, and
    /// persist both.
    pub(super) fn set_recent_limit(&mut self, limit: usize) {
        self.recent_limit = limit.clamp(RECENT_MIN, RECENT_MAX);
        self.recent_files.truncate(self.recent_limit);
        self.save_config();
    }
}
