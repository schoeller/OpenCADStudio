// Small platform shims for things the desktop build does natively but the web
// (wasm) build must handle differently or skip.

/// Open a URL in the user's browser. The desktop launches the default handler;
/// the web opens a new tab (the button click is a user gesture, so it isn't
/// caught by the pop-up blocker). Focus of the opened page is left to the
/// OS / browser.
#[cfg(not(target_arch = "wasm32"))]
pub fn open_url(url: &str) {
    let _ = open::that(url);
}

#[cfg(target_arch = "wasm32")]
pub fn open_url(url: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.open_with_url_and_target(url, "_blank");
    }
}

/// Web: read text from the system clipboard via the async Clipboard API.
/// iced's own `clipboard::read` is a no-op on the web (the browser clipboard is
/// async + permission-gated), so the editor paste paths use this instead. The
/// Ctrl+V keypress that drives it is a user gesture, so the read is permitted.
/// Returns `None` when denied, empty, or unsupported.
#[cfg(target_arch = "wasm32")]
pub async fn read_clipboard_text() -> Option<String> {
    let clipboard = web_sys::window()?.navigator().clipboard();
    let value = wasm_bindgen_futures::JsFuture::from(clipboard.read_text())
        .await
        .ok()?;
    value.as_string()
}

/// Turn an `rfd` file handle into a `PathBuf` the rest of the app keys on.
///
/// Desktop returns the real filesystem path. The browser has no path, so we
/// synthesize one from the file name — enough for the app to compile and track
/// the document name; actual byte I/O on the web reads the handle directly
/// (a follow-up).
#[cfg(not(target_arch = "wasm32"))]
pub fn handle_path(h: &rfd::FileHandle) -> std::path::PathBuf {
    let p = h.path().to_path_buf();
    // Every dialog result funnels through here — remember its folder so the
    // next dialog opens where the user left off.
    crate::config::remember_dialog_dir(&p);
    p
}

#[cfg(target_arch = "wasm32")]
pub fn handle_path(h: &rfd::FileHandle) -> std::path::PathBuf {
    std::path::PathBuf::from(h.file_name())
}

/// New async file dialog seeded with the last directory a dialog was used in.
/// All pickers should start from this instead of `rfd::AsyncFileDialog::new()`.
#[cfg(not(target_arch = "wasm32"))]
pub fn file_dialog() -> rfd::AsyncFileDialog {
    let dlg = rfd::AsyncFileDialog::new();
    match crate::config::last_dialog_dir() {
        Some(dir) => dlg.set_directory(dir),
        None => dlg,
    }
}

/// Web: no filesystem paths, nothing to remember.
#[cfg(target_arch = "wasm32")]
pub fn file_dialog() -> rfd::AsyncFileDialog {
    rfd::AsyncFileDialog::new()
}

/// Trigger a browser download of `bytes` as `name`. Builds a Blob, points a
/// hidden `<a download>` at it and clicks it programmatically — because this
/// runs inside the Save button's click (a user gesture), the file downloads
/// immediately with no extra "click to download" link. Web only.
#[cfg(target_arch = "wasm32")]
pub fn download_bytes(name: &str, bytes: &[u8]) {
    use wasm_bindgen::JsCast;
    let Some(window) = web_sys::window() else { return };
    let Some(document) = window.document() else { return };
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array.buffer());
    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence(&parts) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    if let Ok(el) = document.create_element("a") {
        let a: web_sys::HtmlAnchorElement = el.unchecked_into();
        a.set_href(&url);
        a.set_download(name);
        a.click();
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

/// Short platform string for bug reports: OS + architecture on the desktop,
/// the browser user-agent on the web.
#[cfg(not(target_arch = "wasm32"))]
pub fn platform_info() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(target_arch = "wasm32")]
pub fn platform_info() -> String {
    web_sys::window()
        .and_then(|w| w.navigator().user_agent().ok())
        .map(|ua| format!("Web — {ua}"))
        .unwrap_or_else(|| "Web (wasm)".to_string())
}

/// Percent-encode `s` for use in a URL query value (e.g. a GitHub issue
/// `?body=`). Encodes everything outside the unreserved set.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
