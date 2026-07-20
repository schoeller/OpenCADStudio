//! Out-of-process plugin runner logic.
//!
//! This module is used by the separate `ocs_plugin_runner` binary. Keeping the
//! runner code inside `ocs_plugin_api` means the runner binary only needs to
//! depend on this crate and stays in sync with the host's protocol.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::host::{BuiltinPlugin, HostApi, InteractiveCommand, V2ToV3Adapter};
use crate::ipc::client::{InteractiveRegistry, IpcClient, PluginHostApi};
use crate::ipc::protocol::{
    HostRequest, HostResponse, HostToPlugin, InteractiveEvent, PluginToHost, PLUGIN_TOKEN_ENV,
};
use crate::ribbon::owned::OwnedRibbonGroup;

/// Write a log line to stderr and flush it immediately. The runner's stderr is
/// redirected to a temp file by the host; without explicit flushing the crash
/// logs can be lost in the block buffer when the process aborts.
fn runner_log(msg: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{msg}");
    let _ = stderr.flush();
}

#[cfg(all(windows, feature = "host"))]
fn install_windows_exception_filter() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SetUnhandledExceptionFilter, EXCEPTION_POINTERS,
    };

    unsafe extern "system" fn handler(info: *const EXCEPTION_POINTERS) -> i32 {
        use std::io::Write;
        if !info.is_null() && !(*info).ExceptionRecord.is_null() {
            let record = &*(*info).ExceptionRecord;
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(
                stderr,
                "[runner] unhandled native exception: code=0x{:08X}, address={:?}",
                record.ExceptionCode, record.ExceptionAddress
            );
            let _ = stderr.flush();
        }
        // Continue the default search so Windows still creates a crash dump and
        // the process terminates as expected.
        0 // EXCEPTION_CONTINUE_SEARCH
    }

    unsafe {
        SetUnhandledExceptionFilter(Some(handler));
    }
}

macro_rules! runner_log {
    ($($arg:tt)*) => {
        runner_log(&format!($($arg)*))
    };
}

/// Entry point for the plugin runner child process.
///
/// Connects back to the host on `sync_socket_name` and `async_socket_name`,
/// loads the cdylib at `cdylib_path`, and runs the request loop until the host
/// sends `Shutdown`. This function never returns normally; it exits the
/// process on shutdown or fatal error so the child does not fall through to
/// the host's GUI main.
pub fn run(
    sync_socket_name: &str,
    async_socket_name: &str,
    cdylib_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(all(windows, feature = "host"))]
    install_windows_exception_filter();

    std::panic::set_hook(Box::new(|info| {
        use std::io::Write;
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "[runner] panic: {info}");
        if let Some(loc) = info.location() {
            let _ = writeln!(stderr, "[runner] panic location: {loc}");
        }
        let _ = stderr.flush();
    }));

    runner_log!(
        "[runner] starting for {cdylib_path:?} on sync={sync_socket_name} async={async_socket_name}"
    );
    let plugin = unsafe { load_plugin(cdylib_path)? };
    let plugin: Arc<Mutex<Box<dyn BuiltinPlugin>>> = Arc::new(Mutex::new(plugin));
    let interactive: InteractiveRegistry = Arc::new(Mutex::new(HashMap::new()));

    let token = match std::env::var(PLUGIN_TOKEN_ENV) {
        Ok(t) => t,
        Err(_) => {
            runner_log!("[runner] missing {PLUGIN_TOKEN_ENV}; exiting");
            std::process::exit(1);
        }
    };

    let sync_client = IpcClient::connect(sync_socket_name)?;
    let async_client = IpcClient::connect(async_socket_name)?;
    runner_log!("[runner] connected to host");
    sync_client.send_handshake(&token)?;
    async_client.send_handshake(&token)?;

    // Split the async client into a full-duplex client so the async event
    // thread can both receive host async events and perform synchronous
    // request/response host API calls over the async socket.
    let async_client = async_client.split();
    let async_client_for_main = async_client.clone();

    // Spawn the async reader thread. It reads host async events from the async
    // socket and delivers them to the plugin's `on_async_event` handler.
    let plugin_for_async = Arc::clone(&plugin);
    let async_interactive = interactive.clone();
    let _async_thread = std::thread::spawn(move || {
        let mut proxy = PluginHostApi::new(
            // The async event thread does not use the sync socket; reuse the
            // full-duplex async client for both slots. The split client
            // performs real request/response RPC even in async mode.
            async_client.clone(),
            async_client.clone(),
            0,
            async_interactive,
        );
        proxy.set_async_mode(true);
        loop {
            let msg: HostToPlugin = match async_client.recv() {
                Ok(m) => m,
                Err(err) => {
                    runner_log!("[runner] async recv error: {err}");
                    break;
                }
            };
            match msg {
                HostToPlugin::Async(event) => {
                    let mut guard = plugin_for_async.lock().unwrap();
                    let plugin_ref = guard.as_mut();
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        plugin_ref.on_async_event(&mut proxy, event)
                    }));
                }
                HostToPlugin::Request(req) => {
                    runner_log!(
                        "[runner] unexpected HostToPlugin::Request on async socket: {req:?}"
                    );
                }
                HostToPlugin::Response(resp) => {
                    runner_log!(
                        "[runner] unexpected HostToPlugin::Response on async socket: {resp:?}"
                    );
                }
            }
        }
        runner_log!("[runner] async reader thread exiting");
    });

    // Main thread reads the sync socket for synchronous request/response RPC.
    loop {
        let msg: HostToPlugin = sync_client.recv()?;
        runner_log!("[runner] host -> runner (sync): {msg:?}");
        match msg {
            HostToPlugin::Request(req) => {
                let is_shutdown = matches!(req, HostRequest::Shutdown);
                let mut proxy = PluginHostApi::new(
                    sync_client.clone(),
                    async_client_for_main.clone(),
                    0,
                    interactive.clone(),
                );
                let guard = plugin.lock().unwrap();
                let resp = handle_host_request(&**guard, &interactive, &mut proxy, req);
                runner_log!("[runner] runner -> host (sync): {resp:?}");
                sync_client.send(&PluginToHost::Response(resp))?;
                if is_shutdown {
                    break;
                }
            }
            HostToPlugin::Response(_) => {
                // Responses are consumed by PluginHostApi::request synchronously.
                // Reaching here means the host sent a response without a pending
                // plugin request.
                runner_log!("[runner] unexpected HostToPlugin::Response on sync socket");
            }
            HostToPlugin::Async(event) => {
                runner_log!("[runner] unexpected HostToPlugin::Async on sync socket: {event:?}");
            }
        }
    }
    Ok(())
}

fn handle_host_request(
    plugin: &dyn BuiltinPlugin,
    interactive: &InteractiveRegistry,
    host: &mut dyn HostApi,
    req: HostRequest,
) -> HostResponse {
    match req {
        HostRequest::GetManifest => {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.manifest())) {
                Ok(m) => HostResponse::Manifest(m.into()),
                Err(_) => HostResponse::Error("plugin manifest() panicked".to_string()),
            }
        }
        HostRequest::GetRibbon => {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.ribbon())) {
                Ok(groups) => HostResponse::Ribbon(
                    groups
                        .ribbon_groups()
                        .iter()
                        .map(OwnedRibbonGroup::from)
                        .collect(),
                ),
                Err(_) => HostResponse::Error("plugin ribbon() panicked".to_string()),
            }
        }
        HostRequest::GetPanels => {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.panels())) {
                Ok(panels) => HostResponse::Panels(panels),
                Err(_) => HostResponse::Error("plugin panels() panicked".to_string()),
            }
        }
        HostRequest::Dispatch { cmd } => {
            let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                plugin.dispatch(host, &cmd)
            }));
            match handled {
                Ok(b) => HostResponse::Bool(b),
                Err(_) => HostResponse::Error("plugin dispatch panicked".to_string()),
            }
        }
        HostRequest::InteractiveEvent { command_id, event } => {
            let step = {
                let mut registry = interactive.lock().unwrap_or_else(|e| e.into_inner());
                let Some(cmd) = registry.get_mut(&command_id) else {
                    return HostResponse::Error(format!(
                        "unknown interactive command {command_id}"
                    ));
                };
                let cmd_ref: &mut dyn InteractiveCommand = cmd.as_mut();
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match event {
                    InteractiveEvent::Point(pt) => cmd_ref.on_point(pt),
                    InteractiveEvent::Enter => cmd_ref.on_enter(),
                    InteractiveEvent::ObjectPick { handle, pt } => {
                        cmd_ref.on_object_pick(handle, pt)
                    }
                }))
            };
            match step {
                Ok(s) => HostResponse::CommandStep(s),
                Err(_) => HostResponse::Error("interactive command panicked".to_string()),
            }
        }
        HostRequest::GetPrompt { command_id } => {
            let result = {
                let registry = interactive.lock().unwrap_or_else(|e| e.into_inner());
                registry.get(&command_id).map(|cmd| {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cmd.prompt()))
                })
            };
            match result {
                Some(Ok(s)) => HostResponse::Text(s),
                Some(Err(_)) => HostResponse::Error("prompt() panicked".to_string()),
                None => HostResponse::Error(format!("unknown interactive command {command_id}")),
            }
        }
        HostRequest::NeedsEntityPick { command_id } => {
            let result = {
                let registry = interactive.lock().unwrap_or_else(|e| e.into_inner());
                registry.get(&command_id).map(|cmd| {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        cmd.needs_object_pick()
                    }))
                })
            };
            match result {
                Some(Ok(b)) => HostResponse::Bool(b),
                Some(Err(_)) => HostResponse::Error("needs_object_pick() panicked".to_string()),
                None => HostResponse::Error(format!("unknown interactive command {command_id}")),
            }
        }
        HostRequest::Shutdown => {
            // Return a response so the host sees the acknowledgement; the main
            // loop will send it and then exit cleanly.
            HostResponse::Bool(true)
        }
    }
}

unsafe fn load_plugin(path: &Path) -> Result<Box<dyn BuiltinPlugin>, Box<dyn std::error::Error>> {
    let lib = libloading::Library::new(path)?;

    let version: libloading::Symbol<extern "C" fn() -> u32> = lib
        .get(b"ocs_plugin_api_version")
        .map_err(|_| "missing ocs_plugin_api_version symbol")?;
    let v = version();
    runner_log!("[runner] plugin reports API version {v}");
    if !crate::host_accepts_plugin_version(v) {
        return Err(format!(
            "API version {v} is incompatible (host supports {}-{})",
            crate::API_VERSION_MIN_SUPPORTED,
            crate::API_VERSION
        )
        .into());
    }

    let plugin: Box<dyn BuiltinPlugin> = if v == 2 {
        // API v2 cdylibs export `*mut Box<dyn BuiltinPlugin>` using only the
        // first three trait methods (manifest, ribbon, dispatch). Wrap the
        // returned object so the v3 `BuiltinPlugin` view uses safe no-op
        // defaults for panels and async events, masking any incomplete v2 vtable.
        let register: libloading::Symbol<extern "C" fn() -> *mut Box<dyn BuiltinPlugin>> = lib
            .get(b"ocs_plugin_register")
            .map_err(|_| "missing ocs_plugin_register symbol")?;
        let raw = register();
        if raw.is_null() {
            return Err("ocs_plugin_register returned null".into());
        }
        let v2_plugin = *Box::from_raw(raw);
        Box::new(V2ToV3Adapter(v2_plugin))
    } else {
        // v3: check ABI revision before constructing the plugin so a stale v3
        // cdylib cannot crash the runner with a mismatched vtable.
        let revision: libloading::Symbol<extern "C" fn() -> u64> = lib
            .get(b"ocs_plugin_abi_revision")
            .map_err(|_| "missing ocs_plugin_abi_revision symbol")?;
        let plugin_revision = revision();
        if plugin_revision != crate::ABI_REVISION {
            return Err(format!(
                "ABI revision mismatch: plugin {plugin_revision}, host {}",
                crate::ABI_REVISION
            )
            .into());
        }

        let register: libloading::Symbol<extern "C" fn() -> *mut Box<dyn BuiltinPlugin>> = lib
            .get(b"ocs_plugin_register")
            .map_err(|_| "missing ocs_plugin_register symbol")?;
        let raw = register();
        if raw.is_null() {
            return Err("ocs_plugin_register returned null".into());
        }
        *Box::from_raw(raw)
    };

    // Intentionally leak the library so its vtables remain valid for the
    // lifetime of the process. The runner exits when the host disconnects.
    let _ = std::mem::ManuallyDrop::new(lib);

    Ok(plugin)
}
