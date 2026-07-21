//! Integration test for the Python REPL plugin data path.
//!
//! This test sets up a real host document snapshot and mutation queue, builds
//! the `ocs_acadifc` PyO3 extension, runs the benchmark Python script against
//! it, and verifies that the round-trip of adding and removing 1000 points
//! completes in under one second.
//!
//! The test is skipped automatically when Python is not installed.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use acadrust::tables::Layer;
use acadrust::CadDocument;
use interprocess::local_socket::traits::Listener;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
use ocs_plugin_api::shm::{DocumentFullSnapshotStore, DocumentMutationQueue, EntityOp};
use tempfile::TempDir;

const BENCH_SCRIPT: &str = include_str!("bench_roundtrip_1000_points.py");

/// Find or build the `ocs_acadifc` extension. On Windows the built artifact is
/// a `.dll`; it is copied next to the workspace and renamed so Python can
/// import it as `ocs_acadifc`.
fn ensure_extension(workspace: &Path) -> Option<PathBuf> {
    let _ = find_python()?;
    let src = find_or_build_extension()?;

    let dst_name = if cfg!(windows) {
        "ocs.pyd"
    } else {
        "ocs.so"
    };
    let dst = workspace.join(dst_name);
    let _ = std::fs::copy(&src, &dst);
    if dst.exists() {
        Some(dst)
    } else {
        None
    }
}

fn find_python() -> Option<PathBuf> {
    for name in ["python", "python3"] {
        if let Ok(output) = Command::new(name).arg("--version").output() {
            if output.status.success() {
                return Some(PathBuf::from(name));
            }
        }
    }
    None
}

fn find_or_build_extension() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?.parent()?;
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root.join("target"));

    let artifact_name = if cfg!(windows) {
        "ocs_acadifc.dll"
    } else if cfg!(target_os = "macos") {
        "libocs_acadifc.dylib"
    } else {
        "libocs_acadifc.so"
    };
    let artifact = target_dir.join("debug").join(artifact_name);
    if artifact.exists() {
        return Some(artifact);
    }

    // Build the extension if it is not already present.
    let status = Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("ocs_acadifc")
        .current_dir(workspace_root)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    if artifact.exists() {
        Some(artifact)
    } else {
        None
    }
}

fn apply_entity_batch(doc: &mut CadDocument, ops: Vec<EntityOp>) -> (usize, usize) {
    let mut applied = 0;
    let mut failed = 0;
    for op in ops {
        match op {
            EntityOp::Add(entity) => {
                if doc.add_entity(entity).is_ok() {
                    applied += 1;
                } else {
                    failed += 1;
                }
            }
            EntityOp::Update(_entity) => {
                failed += 1;
            }
            EntityOp::Remove(handle) => {
                if doc.remove_entity(handle).is_some() {
                    applied += 1;
                } else {
                    failed += 1;
                }
            }
        }
    }
    (applied, failed)
}

fn unique_socket_name() -> String {
    format!(
        "ocs_repl_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    )
}

fn write_bootstrap(
    path: &Path,
    workspace: &Path,
    snapshot: &Path,
    queue: &Path,
    control_socket: &str,
) -> std::io::Result<()> {
    let script = format!(
        r#"import os, sys
sys.path.insert(0, {workspace:?})

import ocs

ocs._init(
    snapshot_path={full_path:?},
    queue_path={queue_path:?},
    control_socket={control_socket:?},
)

main_py = os.path.join({workspace:?}, "main.py")
with open(main_py) as f:
    code = compile(f.read(), main_py, "exec")
    exec(code, {{"__name__": "__main__"}})
"#,
        workspace = workspace,
        full_path = snapshot,
        queue_path = queue,
        control_socket = control_socket,
    );
    std::fs::write(path, script)
}

#[test]
fn plugin_manifest_is_valid() {
    use ocs_python_repl::MANIFEST;
    assert_eq!(MANIFEST.id, "ocs.python.repl");
    assert_eq!(MANIFEST.command_prefixes, &["PYTHONEDIT"]);
}

#[test]
fn roundtrip_1000_points() {
    let python = match find_python() {
        Some(p) => p,
        None => {
            eprintln!("python not found; skipping roundtrip test");
            return;
        }
    };
    let temp = TempDir::new().expect("temp dir");
    let workspace = temp.path().to_path_buf();

    let _ = match ensure_extension(&workspace) {
        Some(p) => p,
        None => {
            eprintln!("ocs_acadifc extension not available; skipping roundtrip test");
            return;
        }
    };

    // Prepare the host document and shared-memory resources.
    let mut doc = CadDocument::new();
    doc.layers.add(Layer::new("PTS")).unwrap();

    let mut store = DocumentFullSnapshotStore::new(0).expect("snapshot store");
    let queue = DocumentMutationQueue::new(0).expect("mutation queue");
    store.publish(&doc).expect("initial publish");

    let doc = Arc::new(Mutex::new(doc));
    let doc_for_listener = Arc::clone(&doc);
    let store = Arc::new(Mutex::new(store));
    let store_for_listener = Arc::clone(&store);
    let queue = Arc::new(Mutex::new(queue));
    let queue_for_listener = Arc::clone(&queue);

    let control_socket = unique_socket_name();
    let name_ref = control_socket
        .clone()
        .to_ns_name::<GenericNamespaced>()
        .expect("socket name");
    let listener = ListenerOptions::new()
        .name(name_ref)
        .create_sync()
        .expect("listener");

    // Host-side control socket handler: apply queued mutations and publish a
    // new full snapshot each time the Python side commits. The outer loop
    // accepts a fresh connection for each commit because the Python extension
    // opens and closes the control socket per message.
    let _handle = thread::spawn(move || {
        loop {
            let mut stream = match listener.accept() {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
                let ack = if line.trim() == "REFRESH" {
                    let ops = {
                        let mut q = queue_for_listener.lock().unwrap();
                        q.drain(|_| {})
                    };
                    let (applied, failed) = {
                        let mut d = doc_for_listener.lock().unwrap();
                        apply_entity_batch(&mut d, ops)
                    };
                    let _ = applied;
                    let _ = failed;
                    {
                        let mut s = store_for_listener.lock().unwrap();
                        let d = doc_for_listener.lock().unwrap();
                        s.publish(&d).expect("publish snapshot");
                    }
                    "OK\n"
                } else {
                    "ERR\n"
                };
                let _ = reader.get_mut().write_all(ack.as_bytes());
                let _ = reader.get_mut().flush();
            }
        }
    });

    // Write the Python bootstrap and benchmark script into the workspace.
    let bootstrap = workspace.join("_ocs_bootstrap.py");
    let main_py = workspace.join("main.py");
    write_bootstrap(
        &bootstrap,
        &workspace,
        store.lock().unwrap().path(),
        queue.lock().unwrap().path(),
        &control_socket,
    )
    .expect("write bootstrap");
    std::fs::write(&main_py, BENCH_SCRIPT).expect("write main.py");

    // Give the listener a moment to enter accept() before Python tries to
    // connect.
    thread::sleep(Duration::from_millis(100));

    // Run the Python benchmark.
    let start = Instant::now();
    let mut child = Command::new(python)
        .arg("-u")
        .arg(&bootstrap)
        .current_dir(&workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python");

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let stdout_lines = Arc::new(Mutex::new(String::new()));
    let stderr_lines = Arc::new(Mutex::new(String::new()));
    let stdout_lines_c = Arc::clone(&stdout_lines);
    let stderr_lines_c = Arc::clone(&stderr_lines);
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            stdout_lines_c.lock().unwrap().push_str(&line);
            line.clear();
        }
    });
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            stderr_lines_c.lock().unwrap().push_str(&line);
            line.clear();
        }
    });

    let mut timed_out = false;
    let deadline = start + Duration::from_secs(60);
    loop {
        match child.try_wait().unwrap() {
            Some(_status) => break,
            None => {
                if Instant::now() > deadline {
                    timed_out = true;
                    let _ = child.kill();
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    let status = child.wait().ok();
    let wall = start.elapsed();

    // Give the reader threads a moment to drain the closed pipes.
    thread::sleep(Duration::from_millis(100));
    let stdout_lines = stdout_lines.lock().unwrap().clone();
    let stderr_lines = stderr_lines.lock().unwrap().clone();

    // Give the listener a moment to finish applying any pending REFRESH before
    // we check the final document state. The listener may still be blocked in
    // accept() if Python exited early, in which case we detach it.
    thread::sleep(Duration::from_millis(200));

    eprintln!("--- python stdout ---\n{stdout_lines}");
    if !stderr_lines.is_empty() {
        eprintln!("--- python stderr ---\n{stderr_lines}");
    }

    assert!(!timed_out, "python benchmark timed out; stderr: {stderr_lines}");
    assert!(
        status.map(|s| s.success()).unwrap_or(false),
        "python benchmark exited with non-zero status: {status:?}\nstderr: {stderr_lines}"
    );

    let final_doc = doc.lock().unwrap();
    assert_eq!(
        final_doc.entities().count(),
        0,
        "document should be empty after roundtrip"
    );

    // The Python script already asserts the total < 1.0s; we also print the
    // wall-clock time measured by the host for diagnostics.
    println!("roundtrip wall time: {:.3}s", wall.as_secs_f64());
    assert!(
        wall < Duration::from_secs(5),
        "host-measured roundtrip took too long: {wall:?}"
    );
}

#[test]
fn entity_constructors_load() {
    let python = match find_python() {
        Some(p) => p,
        None => {
            eprintln!("python not found; skipping entity_constructors_load test");
            return;
        }
    };
    let temp = TempDir::new().expect("temp dir");
    let workspace = temp.path();

    let _ = match ensure_extension(workspace) {
        Some(p) => p,
        None => {
            eprintln!("ocs_acadifc extension not available; skipping entity_constructors_load test");
            return;
        }
    };

    let script = workspace.join("constructors.py");
    std::fs::write(
        &script,
        r#"import ocs

v = ocs.Vector3(1, 2, 3)
assert v.x == 1.0
p = ocs.Point(1, 2, 3, layer="PTS")
assert p.layer == "PTS"
l = ocs.Line(ocs.Vector3(0, 0, 0), ocs.Vector3(1, 1, 1))
c = ocs.Circle(ocs.Vector3(0, 0, 0), 5)
a = ocs.Arc(ocs.Vector3(0, 0, 0), 5, 0, 1.57)
t = ocs.Text("hello", 0, 0, 0, height=2.5)
layer = ocs.Layer("TEST")
assert layer.name == "TEST"
print("constructors ok")
"#,
    )
    .expect("write script");

    let status = Command::new(python)
        .arg("-u")
        .arg(&script)
        .current_dir(workspace)
        .status()
        .expect("spawn python");

    assert!(status.success(), "python constructor test failed");
}
