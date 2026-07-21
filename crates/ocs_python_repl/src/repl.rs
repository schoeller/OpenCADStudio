//! Python REPL process management and async event forwarding.

use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use interprocess::local_socket::traits::Listener;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Stream, ToNsName};

use ocs_plugin_api::host::{HostApi, PluginAsyncSender};
use ocs_plugin_api::ipc::protocol::PluginAsync;
use ocs_plugin_api::shm::{DocumentFullSnapshotInfo, DocumentFullSnapshotReader, DocumentMutationQueueInfo};

pub struct ReplSession {
    child: Child,
}

impl ReplSession {
    /// Spawn a Python REPL child and start a background thread that forwards
    /// control messages to the host as `PluginAsync` events.
    pub fn spawn<F>(
        python: &Path,
        workspace: &Path,
        host: &mut dyn HostApi,
        _status: F,
    ) -> io::Result<Self>
    where
        F: FnOnce(String) + Send + 'static,
    {
        let Some(sender) = host.async_sender() else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "async sender unavailable; Python REPL requires an out-of-process host",
            ));
        };

        let full = match host.document_full_snapshot() {
            Some(info) => info,
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "document full snapshot unavailable",
                ))
            }
        };
        let queue = match host.document_mutation_queue() {
            Some(info) => info,
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "document mutation queue unavailable",
                ))
            }
        };

        let control_socket_name = unique_socket_name();
        let name_ref: interprocess::local_socket::Name = control_socket_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let listener = ListenerOptions::new().name(name_ref).create_sync()?;

        let extension = find_extension()?;
        let bootstrap = workspace.join("_ocs_bootstrap.py");
        write_bootstrap(&bootstrap, workspace, &extension, &full, &queue, &control_socket_name)?;

        // Write a config file so the script can self-initialize when run directly
        // (e.g. by Zed's debugpy runner) without going through the bootstrap.
        let config = workspace.join("_ocs_config.json");
        let config_json = serde_json::json!({
            "snapshot_path": full.path,
            "queue_path": queue.path,
            "control_socket": control_socket_name,
        });
        std::fs::write(&config, serde_json::to_string_pretty(&config_json)?)?;

        // Windows: copy the extension to the workspace so Python can load it by name.
        #[cfg(windows)]
        {
            let ext_name = extension
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let _ = std::fs::copy(&extension, workspace.join(&ext_name));
        }

        let mut child = Command::new(python)
            .arg("-u")
            .arg(&bootstrap)
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("OCS_PYTHON_WORKSPACE", workspace)
            .env("OCS_PYTHON_EXTENSION", &extension)
            .env("OCS_FULL_SNAPSHOT", &full.path)
            .env("OCS_MUTATION_QUEUE", &queue.path)
            .env("OCS_CONTROL_SOCKET", &control_socket_name)
            .spawn()?;

        // Forward stdout/stderr to a temp log so they are not lost.
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        let log = workspace.join("_ocs_python.log");
        thread::spawn(move || forward_logs(stdout, stderr, &log));

        // Background thread: accept the Python control socket and forward events.
        let control_socket = control_socket_name.clone();
        let full_path = PathBuf::from(&full.path);
        thread::spawn(move || {
            loop {
                match listener.accept() {
                    Ok(stream) => {
                        let sender = Arc::clone(&sender);
                        let path = full_path.clone();
                        forward_control(stream, sender, &path);
                    }
                    Err(_) => break,
                }
            }
            // Remove the control socket namespace when done.
            let _ = std::fs::remove_file(format!("\\{}", control_socket));
        });

        Ok(Self {
            child,
        })
    }

    #[allow(dead_code)]
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn shutdown(&mut self) -> io::Result<()> {
        let _ = self.child.kill();
        self.child.wait()?;
        Ok(())
    }
}

impl Drop for ReplSession {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn forward_logs(stdout: std::process::ChildStdout, stderr: std::process::ChildStderr, log: &Path) {
    let mut out = BufReader::new(stdout);
    let mut err = BufReader::new(stderr);
    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        Ok(f) => f,
        Err(_) => return,
    };
    let mut line = String::new();
    loop {
        line.clear();
        let Ok(n) = out.read_line(&mut line) else { break; };
        if n == 0 && line.is_empty() {
            break;
        }
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
    loop {
        line.clear();
        let Ok(n) = err.read_line(&mut line) else { break; };
        if n == 0 && line.is_empty() {
            break;
        }
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

fn forward_control(stream: Stream, sender: Arc<dyn PluginAsyncSender>, full_path: &Path) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF: client closed the connection.
            Ok(_) if line.trim().is_empty() => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[python-repl] control socket read error: {e}");
                break;
            }
        }

        let ack = match line.trim() {
            "REFRESH" => {
                // Read the version before asking the host to apply so we can
                // wait until the snapshot actually changes before replying OK.
                let start_version = DocumentFullSnapshotReader::open(full_path)
                    .map(|r| r.version())
                    .unwrap_or(0);
                if let Err(e) = sender.send(PluginAsync::DocumentRefreshRequested) {
                    eprintln!("[python-repl] failed to forward REFRESH: {e}");
                    "ERR\n"
                } else {
                    let start = std::time::Instant::now();
                    let timeout = Duration::from_secs(30);
                    let mut current_version = start_version;
                    while current_version == start_version && start.elapsed() < timeout {
                        if let Ok(r) = DocumentFullSnapshotReader::open(full_path) {
                            current_version = r.version();
                        }
                        if current_version == start_version {
                            thread::sleep(Duration::from_millis(1));
                        }
                    }
                    if current_version == start_version {
                        eprintln!("[python-repl] REFRESH timeout waiting for snapshot version change");
                    }
                    "OK\n"
                }
            }
            other => {
                eprintln!("[python-repl] unknown control message: {other}");
                "ERR\n"
            }
        };
        let _ = reader.get_mut().write_all(ack.as_bytes());
        let _ = reader.get_mut().flush();
    }
}

fn unique_socket_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("ocs_python_repl_{}_{}", std::process::id(), n)
}

fn find_extension() -> io::Result<PathBuf> {
    if let Ok(path) = std::env::var("OCS_ACADIFC_EXTENSION") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
    }

    let ext = if cfg!(windows) {
        "ocs.pyd"
    } else if cfg!(target_os = "macos") {
        "ocs.so"
    } else {
        "ocs.so"
    };

    // The plugin runs inside the plugin runner process, so current_exe is the runner.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(ext);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "cannot find {ext}; build the ocs_acadifc extension and rename/copy it to ocs.pyd (or set OCS_ACADIFC_EXTENSION)"
        ),
    ))
}

fn write_bootstrap(
    path: &Path,
    workspace: &Path,
    extension: &Path,
    full: &DocumentFullSnapshotInfo,
    queue: &DocumentMutationQueueInfo,
    control_socket: &str,
) -> io::Result<()> {
    let ext_dir = extension.parent().unwrap_or(Path::new("."));
    let bootstrap = format!(
        r#"import os, sys, site, socket
sys.path.insert(0, {ext_dir:?})

# Import the Rust extension. The artifact must be named ocs.pyd (Windows) or
# ocs.so (Unix) so Python can load it as `ocs`.
import ocs

# Wire shared memory paths and control socket.
ocs._init(
    snapshot_path={full_path:?},
    queue_path={queue_path:?},
    control_socket={control_socket:?},
)

# Start debugpy listener if available.
try:
    import debugpy
    debugpy.listen(5678)
except Exception as e:
    print(f"debugpy not available: {{e}}")

main_py = os.path.join({workspace:?}, "main.py")
if os.path.exists(main_py):
    with open(main_py) as f:
        code = compile(f.read(), main_py, "exec")
        exec(code, {{"__name__": "__main__"}})
else:
    # Fallback: interactive prompt is not available in this mode, so keep the
    # process alive reading from the control socket.
    import time
    while True:
        time.sleep(1)
"#,
        ext_dir = ext_dir,
        full_path = full.path,
        queue_path = queue.path,
        control_socket = control_socket,
        workspace = workspace,
    );
    std::fs::write(path, bootstrap)
}
