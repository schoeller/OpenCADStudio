//! Out-of-process plugin runner logic.
//!
//! This module is used by the host when it spawns itself in runner mode
//! (`--ocs-plugin-runner <socket> <cdylib>`). Keeping the runner code inside
//! `ocs_plugin_api` means the host only needs to know the CLI contract, not the
//! internal plugin-loading and IPC details.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::host::{BuiltinPlugin, InteractiveCommand};
use crate::ipc::protocol::{HostRequest, HostResponse};
use crate::ipc::client::{
    InteractiveRegistry, InteractiveRegistryV3, IpcClient, IpcClientV3, PluginHostApi,
    PluginHostApiV3,
};
use crate::ipc::protocol::{
    HostToPlugin, InteractiveEvent, PluginToHost, PLUGIN_TOKEN_ENV,
};
use crate::ipc::transport::{recv, send};
use crate::ribbon::owned::OwnedRibbonGroup;

/// Entry point for the plugin runner child process.
///
/// Connects back to the host on `socket_name`, loads the cdylib at
/// `cdylib_path`, and runs the request loop until the host sends `Shutdown`.
/// This function never returns normally; it exits the process on shutdown or
/// fatal error so the child does not fall through to the host's GUI main.
pub fn run(socket_name: &str, cdylib_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("[runner] starting for {cdylib_path:?} on {socket_name}");
    let (plugin, _api_version) = unsafe { load_plugin(cdylib_path)? };
    let plugin: Arc<dyn BuiltinPlugin> = Arc::from(plugin);

    let token = match std::env::var(PLUGIN_TOKEN_ENV) {
        Ok(t) => t,
        Err(_) => {
            eprintln!("[runner] missing {PLUGIN_TOKEN_ENV}; exiting");
            std::process::exit(1);
        }
    };

    let plugin_api_version = plugin.manifest().api_version.major;
    if plugin_api_version >= 3 {
        run_v3(socket_name, &token, plugin)
    } else {
        run_v2(socket_name, &token, plugin)
    }
}

fn run_v2(
    socket_name: &str,
    token: &str,
    plugin: Arc<dyn BuiltinPlugin>,
) -> Result<(), Box<dyn std::error::Error>> {
    let interactive: InteractiveRegistry = Rc::new(RefCell::new(HashMap::new()));

    let client = IpcClient::connect(socket_name)?;
    eprintln!("[runner v2] connected to host");
    client.send_handshake(token)?;

    loop {
        let msg: HostToPlugin = recv(&mut client.stream_ref())?;
        eprintln!("[runner v2] host -> runner: {msg:?}");
        match msg {
            HostToPlugin::Request(req) => {
                let resp = handle_host_request(&*plugin, &interactive, &client, req);
                eprintln!("[runner v2] runner -> host: {resp:?}");
                send(&mut client.stream_ref(), &PluginToHost::Response(resp))?;
            }
            HostToPlugin::Response(_) => {
                eprintln!("[runner v2] unexpected HostToPlugin::Response");
            }
            HostToPlugin::RequestV3 { .. } | HostToPlugin::ResponseV3 { .. } => {
                eprintln!("[runner v2] unexpected API v3 message");
            }
        }
    }
}

fn run_v3(
    socket_name: &str,
    token: &str,
    plugin: Arc<dyn BuiltinPlugin>,
) -> Result<(), Box<dyn std::error::Error>> {
    let interactive: InteractiveRegistryV3 = Arc::new(Mutex::new(HashMap::new()));
    let client = IpcClientV3::connect(socket_name)?;
    eprintln!("[runner v3] connected to host");
    client.send_handshake(token)?;

    let stream = client.clone();
    let reader_plugin = Arc::clone(&plugin);
    let _reader = std::thread::spawn(move || {
        let interactive = Arc::clone(&interactive);
        let plugin = reader_plugin;
        loop {
            let msg = match recv::<HostToPlugin>(&mut *stream.stream.lock().unwrap()) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("[runner v3] recv error: {e}");
                    break;
                }
            };
            eprintln!("[runner v3] host -> runner: {msg:?}");
            match msg {
                HostToPlugin::RequestV3 { request_id, session_id, request } => {
                    let response = handle_host_request_v3(
                        &*plugin,
                        &client,
                        session_id,
                        request,
                        &interactive,
                    );
                    eprintln!("[runner v3] runner -> host: {response:?}");
                    let _ = send(
                        &mut *stream.stream.lock().unwrap(),
                        &PluginToHost::ResponseV3 { request_id, response },
                    );
                }
                HostToPlugin::ResponseV3 { request_id: _, response } => {
                    // Responses to plugin-initiated requests are consumed by
                    // IpcClientV3::request on the thread that sent the request.
                    eprintln!("[runner v3] unexpected host response: {response:?}");
                }
                HostToPlugin::Request(req) => {
                    // The host currently performs the initial manifest/ribbon
                    // handshake using V2 envelopes, even for V3 plugins. Handle
                    // those here and error on any other V2 request.
                    let response = match req {
                        HostRequest::GetManifest => {
                            HostResponse::Manifest((plugin.manifest()).into())
                        }
                        HostRequest::GetRibbon => HostResponse::Ribbon(
                            plugin
                                .ribbon()
                                .ribbon_groups()
                                .into_iter()
                                .map(OwnedRibbonGroup::from)
                                .collect(),
                        ),
                        HostRequest::Shutdown => {
                            plugin.shutdown();
                            std::process::exit(0);
                        }
                        other => HostResponse::Error(format!(
                            "unexpected V2 request on V3 runner: {other:?}"
                        )),
                    };
                    let _ = send(
                        &mut *stream.stream.lock().unwrap(),
                        &PluginToHost::Response(response),
                    );
                }
                HostToPlugin::Response(resp) => {
                    eprintln!("[runner v3] unexpected V2 response: {resp:?}");
                }
            }
        }
    });

    plugin.run_on_main_thread()?;
    plugin.shutdown();
    // Do not wait for the reader thread: it is blocked on the open stream and
    // the host will close it (or kill the process) when it shuts the plugin
    // down. Exiting the main thread terminates the runner process.
    std::process::exit(0);
}

fn handle_host_request(
    plugin: &dyn BuiltinPlugin,
    interactive: &InteractiveRegistry,
    client: &IpcClient,
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
                        .into_iter()
                        .map(OwnedRibbonGroup::from)
                        .collect(),
                ),
                Err(_) => HostResponse::Error("plugin ribbon() panicked".to_string()),
            }
        }
        HostRequest::Dispatch { cmd } => {
            // The host supplies the active tab index as part of the dispatch
            // context. We cache it inside PluginHostApi.
            let mut proxy = PluginHostApi::new(client.clone(), 0, interactive.clone());
            let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                plugin.dispatch(&mut proxy, &cmd)
            }));
            match handled {
                Ok(b) => HostResponse::Bool(b),
                Err(_) => HostResponse::Error("plugin dispatch panicked".to_string()),
            }
        }
        HostRequest::InteractiveEvent { command_id, event } => {
            let step = {
                let mut registry = interactive.borrow_mut();
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
                let registry = interactive.borrow();
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
                let registry = interactive.borrow();
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
        HostRequest::EndAsyncSession { .. } => {
            HostResponse::Error("async sessions require API v3".to_string())
        }
        HostRequest::AsyncSessionRequest { .. } => {
            HostResponse::Error("async session requests require API v3".to_string())
        }
        HostRequest::Shutdown => {
            // The runner will exit after this response is sent.
            std::process::exit(0);
        }
    }
}

fn handle_host_request_v3(
    plugin: &dyn BuiltinPlugin,
    client: &IpcClientV3,
    _session_id: String,
    req: HostRequest,
    interactive: &InteractiveRegistryV3,
) -> HostResponse {
    match req {
        HostRequest::GetManifest => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.manifest())) {
            Ok(m) => HostResponse::Manifest(m.into()),
            Err(_) => HostResponse::Error("plugin manifest() panicked".to_string()),
        },
        HostRequest::GetRibbon => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.ribbon())) {
            Ok(groups) => HostResponse::Ribbon(
                groups
                    .ribbon_groups()
                    .into_iter()
                    .map(OwnedRibbonGroup::from)
                    .collect(),
            ),
            Err(_) => HostResponse::Error("plugin ribbon() panicked".to_string()),
        },
        HostRequest::Dispatch { cmd } => {
            let mut proxy = PluginHostApiV3::new(client.clone(), 0, Arc::clone(interactive));
            let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                plugin.dispatch(&mut proxy, &cmd)
            }));
            match handled {
                Ok(b) => HostResponse::Bool(b),
                Err(_) => HostResponse::Error("plugin dispatch() panicked".to_string()),
            }
        }
        HostRequest::InteractiveEvent { command_id, event } => {
            let step = {
                let mut registry = interactive.lock().unwrap();
                let Some(cmd) = registry.get_mut(&command_id) else {
                    return HostResponse::Error(format!("unknown interactive command {command_id}"));
                };
                let cmd_ref: &mut dyn InteractiveCommand = cmd.as_mut();
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match event {
                    InteractiveEvent::Point(pt) => cmd_ref.on_point(pt),
                    InteractiveEvent::Enter => cmd_ref.on_enter(),
                    InteractiveEvent::ObjectPick { handle, pt } => cmd_ref.on_object_pick(handle, pt),
                }))
            };
            match step {
                Ok(s) => HostResponse::CommandStep(s),
                Err(_) => HostResponse::Error("interactive command panicked".to_string()),
            }
        }
        HostRequest::GetPrompt { command_id } => {
            let result = {
                let registry = interactive.lock().unwrap();
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
                let registry = interactive.lock().unwrap();
                registry.get(&command_id).map(|cmd| {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cmd.needs_object_pick()))
                })
            };
            match result {
                Some(Ok(b)) => HostResponse::Bool(b),
                Some(Err(_)) => HostResponse::Error("needs_object_pick() panicked".to_string()),
                None => HostResponse::Error(format!("unknown interactive command {command_id}")),
            }
        }
        HostRequest::EndAsyncSession { session_id: req_session_id } => {
            match plugin.on_host_request(&HostRequest::EndAsyncSession { session_id: req_session_id }) {
                Some(resp) => resp,
                None => HostResponse::Bool(true),
            }
        }
        HostRequest::AsyncSessionRequest { request } => {
            let req = HostRequest::AsyncSessionRequest { request };
            match plugin.on_host_request(&req) {
                Some(resp) => resp,
                None => HostResponse::Bool(true),
            }
        }
        HostRequest::Shutdown => {
            std::process::exit(0);
        }
    }
}

unsafe fn load_plugin(path: &Path) -> Result<(Box<dyn BuiltinPlugin>, u32), Box<dyn std::error::Error>> {
    let lib = libloading::Library::new(path)?;

    let version: libloading::Symbol<extern "C" fn() -> u32> = lib
        .get(b"ocs_plugin_api_version")
        .map_err(|_| "missing ocs_plugin_api_version symbol")?;
    let v = version();
    if !crate::host_accepts_plugin_version(v) {
        return Err(format!(
            "API version {v} is incompatible (host supports {}-{})",
            crate::API_VERSION_MIN_SUPPORTED,
            crate::API_VERSION
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
    let plugin = *Box::from_raw(raw);

    // Intentionally leak the library so its vtables remain valid for the
    // lifetime of the process. The runner exits when the host disconnects.
    let _ = std::mem::ManuallyDrop::new(lib);

    Ok((plugin, v))
}
