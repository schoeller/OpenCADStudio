// Open CAD Studio plugin runtime. Plugins are external cdylibs loaded from the
// user plugins folder (see `external`) and installed via the marketplace; the
// host ships no built-in add-ons. See `docs/plugin-architecture.md`.

pub mod external;
pub mod host;
pub mod marketplace;
pub mod panels;
pub mod registry;

pub use registry::{all_ribbon_modules, plugin_command_names, ribbon_modules_enabled};
pub(crate) use registry::try_dispatch;

/// Run a plugin entry point under a panic guard so a buggy external plugin
/// can't take down the host. Returns `None` (after logging) when the plugin
/// panicked; the caller substitutes a safe default. Catches Rust panics across
/// the cdylib boundary — it does NOT contain genuine UB (a plugin that
/// segfaults or violates memory safety via `unsafe` can still crash the host;
/// only out-of-process isolation would). (#145)
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_string());
            let line = format!("Plugin {what} panicked: {msg} — call ignored");
            eprintln!("[plugin] {line}");
            // The plugin hooks run deep inside the update / render loop where
            // the app's command line isn't reachable, so queue the error and
            // let the next update tick drain it into the command line. (#145)
            errors().lock().unwrap().push(line);
            None
        }
    }
}

/// Process-wide queue of plugin-guard errors waiting to surface in the host's
/// command-line history. Flushed by [`drain_errors`] from the app loop.
#[cfg(not(target_arch = "wasm32"))]
fn errors() -> &'static std::sync::Mutex<Vec<String>> {
    static E: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();
    E.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Take and return every queued plugin-guard error since the last drain.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn drain_errors() -> Vec<String> {
    std::mem::take(&mut *errors().lock().unwrap())
}

#[cfg(test)]
mod tests {
    use super::guard;

    #[test]
    fn returns_value_on_success() {
        let r = guard("ok", || 42);
        assert_eq!(r, Some(42));
    }

    #[test]
    fn swallows_panic_and_queues_error_message() {
        let _ = super::drain_errors(); // clear anything from earlier tests
        // Silence the default panic-hook stderr noise during the test (the
        // guard's own eprintln stays — that's the diagnostic we want).
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r: Option<i32> = guard("boom", || panic!("plugin went wrong"));
        std::panic::set_hook(prev);
        assert_eq!(r, None);
        // The user-visible message lands in the plugin error queue (the app
        // loop drains it into the command line on the next tick).
        let queued = super::drain_errors();
        assert!(
            queued.iter().any(|m| m.contains("boom") && m.contains("plugin went wrong")),
            "expected an error queued for the command line, got: {queued:?}"
        );
    }
}

// ── Public plugin lifecycle hooks (thin delegation to the panel manager) ────

/// The active document tab changed. Forwarded to every plugin process that
/// owns an open panel.
pub(crate) fn on_document_activated(app: &mut crate::app::OpenCADStudio, tab: usize) {
    let _ = app;
    #[cfg(not(target_arch = "wasm32"))]
    crate::plugin::external::with_manager(|mgr| {
        mgr.broadcast_document_event(tab, crate::plugin::panels::DocumentEvent::Activated);
    });
}

pub(crate) fn on_document_changed(app: &mut crate::app::OpenCADStudio, tab: usize) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let version = app.geometry_epoch(tab);
        crate::plugin::external::with_manager(|mgr| {
            mgr.broadcast_document_event(tab, crate::plugin::panels::DocumentEvent::Changed { version });
        });
    }
}

pub(crate) fn on_tab_closed(app: &mut crate::app::OpenCADStudio, tab: usize) {
    let _ = app;
    #[cfg(not(target_arch = "wasm32"))]
    crate::plugin::external::with_manager(|mgr| {
        mgr.broadcast_document_event(tab, crate::plugin::panels::DocumentEvent::Closed);
    });
}

pub(crate) fn on_message(
    app: &mut crate::app::OpenCADStudio,
    msg: &crate::app::Message,
) -> Option<iced::Task<crate::app::Message>> {
    let _ = app;
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::plugin::external::with_manager(|mgr| mgr.handle_message(msg))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

fn host_log(_msg: &str) {
    // Disabled in release builds to avoid synchronous file I/O on the async
    // drain path. Re-enable locally when debugging plugin IPC.
}

pub(crate) fn drain_async_events(app: &mut crate::app::OpenCADStudio) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        host_log("drain_async_events start");
        let active_tab = app.active_tab();
        let mut host = crate::app::plugin_host::HostSession::new(app, active_tab);
        crate::plugin::external::with_manager(|mgr| mgr.drain_and_handle_async(&mut host));
        host_log("drain_async_events end");
    }
    let _ = app;
}
