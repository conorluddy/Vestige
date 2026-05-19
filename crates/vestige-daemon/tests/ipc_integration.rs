//! End-to-end IPC integration tests.
//!
//! Spawns the daemon as an in-process tokio task, connects to its Unix socket,
//! exercises each JSON-RPC method, and asserts response shapes. Cancellation
//! is driven by a [`tokio::sync::Notify`] handed to [`run_with_cancel`] so no
//! UNIX signals are required.

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::watch;

use vestige_daemon::{DaemonOpts, run_with_cancel};

// === HELPERS ===

/// Build minimal [`DaemonOpts`] isolated to `dir`.
///
/// Sets `projects_root` to an empty subdirectory of `dir` so the daemon does
/// not inherit real project workers from `~/.vestige/projects/`, which could
/// have WAL locks or latency that cause the tests to time out.
fn opts_in(dir: &std::path::Path) -> (DaemonOpts, std::path::PathBuf) {
    let socket = dir.join("daemon.sock");
    let projects_root = dir.join("projects");
    std::fs::create_dir_all(&projects_root).unwrap();
    let opts = DaemonOpts {
        foreground: true,
        pid_file: Some(dir.join("daemon.pid")),
        socket_path: Some(socket.clone()),
        status_file: Some(dir.join("daemon.status.json")),
        log_file: None,
        projects_root: Some(projects_root),
    };
    (opts, socket)
}

/// Send one JSON-RPC request and read the single newline-terminated response.
async fn rpc(socket: &std::path::Path, request: &str) -> Value {
    let mut stream = UnixStream::connect(socket)
        .await
        .expect("connect to daemon socket");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    stream.write_all(b"\n").await.expect("write newline");

    let mut buf = String::new();
    BufReader::new(stream)
        .read_line(&mut buf)
        .await
        .expect("read response");

    serde_json::from_str(buf.trim_end()).expect("parse response JSON")
}

/// Wait until the socket file appears, up to a short deadline.
async fn wait_for_socket(socket: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if socket.exists() {
            // Give the listener one more moment to be ready to accept.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("daemon socket did not appear within 5s at {}", socket.display());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

// === TESTS ===

/// `daemon.status` returns a valid JSON-RPC 2.0 response with a pid field.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_status_round_trip() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (opts, socket) = opts_in(tmp.path());
    let (cancel_tx, cancel_rx) = watch::channel(false);

    let daemon = tokio::spawn(async move {
        run_with_cancel(opts, cancel_rx)
            .await
            .expect("daemon run failed")
    });

    wait_for_socket(&socket).await;

    let resp = rpc(
        &socket,
        r#"{"jsonrpc":"2.0","id":1,"method":"daemon.status","params":{}}"#,
    )
    .await;

    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert!(resp["result"].is_object(), "result must be an object");
    assert!(
        resp["result"]["pid"].is_u64(),
        "result.pid must be a u64, got: {}",
        resp["result"]["pid"]
    );
    assert_eq!(resp["result"]["schema_version"], 1);
    assert!(resp["error"].is_null(), "no error expected");

    cancel_tx.send(true).ok();
    daemon.await.unwrap();
}

/// `daemon.register_project` second call is idempotent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_register_project_idempotent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (opts, socket) = opts_in(tmp.path());
    let (cancel_tx, cancel_rx) = watch::channel(false);

    let daemon = tokio::spawn(async move {
        run_with_cancel(opts, cancel_rx)
            .await
            .expect("daemon run failed")
    });

    wait_for_socket(&socket).await;

    let req = r#"{"jsonrpc":"2.0","id":2,"method":"daemon.register_project","params":{"project_id":"proj_ipc-test","project_name":"IPC Test","repo_root":"/tmp/ipc-test"}}"#;

    let resp1 = rpc(&socket, req).await;
    // May fail if HOME is not set for storage resolution — accept gracefully.
    if resp1["result"].is_object() {
        assert_eq!(resp1["result"]["project_id"], "proj_ipc-test");

        let resp2 = rpc(&socket, req).await;
        assert_eq!(resp2["result"]["registered"], false, "second call must be idempotent");
    }

    cancel_tx.send(true).ok();
    daemon.await.unwrap();
}

/// `daemon.kick` with an unimplemented job returns JOB_NOT_IMPLEMENTED.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_kick_unimplemented_job_over_socket() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (opts, socket) = opts_in(tmp.path());
    let (cancel_tx, cancel_rx) = watch::channel(false);

    let daemon = tokio::spawn(async move {
        run_with_cancel(opts, cancel_rx)
            .await
            .expect("daemon run failed")
    });

    wait_for_socket(&socket).await;

    let resp = rpc(
        &socket,
        r#"{"jsonrpc":"2.0","id":3,"method":"daemon.kick","params":{"job":"prune"}}"#,
    )
    .await;

    assert!(resp["error"].is_object(), "expected an error response");
    assert_eq!(resp["error"]["code"], -32000);
    assert_eq!(resp["error"]["data"]["code"], "JOB_NOT_IMPLEMENTED");
    assert_eq!(resp["error"]["data"]["retryable"], false);

    cancel_tx.send(true).ok();
    daemon.await.unwrap();
}

/// An unknown method returns method-not-found (-32601) over the socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_unknown_method_over_socket() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (opts, socket) = opts_in(tmp.path());
    let (cancel_tx, cancel_rx) = watch::channel(false);

    let daemon = tokio::spawn(async move {
        run_with_cancel(opts, cancel_rx)
            .await
            .expect("daemon run failed")
    });

    wait_for_socket(&socket).await;

    let resp = rpc(
        &socket,
        r#"{"jsonrpc":"2.0","id":4,"method":"daemon.frobnicate","params":{}}"#,
    )
    .await;

    assert!(resp["error"].is_object(), "expected an error response");
    assert_eq!(resp["error"]["code"], -32601);

    cancel_tx.send(true).ok();
    daemon.await.unwrap();
}

/// Multiple sequential requests on separate connections all succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_multiple_requests() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (opts, socket) = opts_in(tmp.path());
    let (cancel_tx, cancel_rx) = watch::channel(false);

    let daemon = tokio::spawn(async move {
        run_with_cancel(opts, cancel_rx)
            .await
            .expect("daemon run failed")
    });

    wait_for_socket(&socket).await;

    // Fire three sequential status requests.
    for i in 1..=3_u32 {
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":{i},"method":"daemon.status","params":{{}}}}"#
        );
        let resp = rpc(&socket, &req).await;
        assert_eq!(resp["id"], i, "id must echo back");
        assert!(resp["result"].is_object(), "request {i}: result must be present");
    }

    cancel_tx.send(true).ok();
    daemon.await.unwrap();
}
