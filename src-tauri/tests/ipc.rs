//! End-to-end tests of the command surface through Tauri's IPC layer (mock runtime):
//! the real app builder, setup, state and serde, invoked with the exact argument
//! shapes the frontend sends (camelCase keys).

use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
use tauri::webview::{InvokeRequest, WebviewWindow};

struct Harness {
    _app: tauri::App<MockRuntime>,
    win: WebviewWindow<MockRuntime>,
    _config: tempfile::TempDir,
}

fn harness() -> Harness {
    // The config dir is read during setup; serialize env + build so parallel tests
    // each get their own directory.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let config = tempfile::tempdir().unwrap();
    // SAFETY: guarded by LOCK; nothing else in this binary reads these concurrently.
    unsafe {
        std::env::set_var("FIVEFILES_CONFIG_DIR", config.path());
        std::env::set_var("FIVEFILES_NO_KEYCHAIN", "1");
    }
    let mut app = fivefiles_lib::build(mock_builder()).build(tauri::generate_context!(test = true)).unwrap();
    fivefiles_lib::init(&mut app).unwrap();
    let win = tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();
    Harness { _app: app, win, _config: config }
}

impl Harness {
    fn call(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        get_ipc_response(
            &self.win,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                // Same origin the bundled frontend runs on (tauri://localhost on macOS/Linux,
                // http://tauri.localhost on Windows).
                url: if cfg!(windows) { "http://tauri.localhost" } else { "tauri://localhost" }.parse().unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|b| b.deserialize::<Value>().unwrap())
    }

    fn raw(&self, cmd: &str, body: Vec<u8>, headers: &[(&'static str, &str)]) -> Result<Value, Value> {
        let mut map = tauri::http::HeaderMap::new();
        for (k, v) in headers {
            map.insert(*k, v.parse().unwrap());
        }
        get_ipc_response(
            &self.win,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: if cfg!(windows) { "http://tauri.localhost" } else { "tauri://localhost" }.parse().unwrap(),
                body: InvokeBody::Raw(body),
                headers: map,
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|b| b.deserialize::<Value>().unwrap())
    }

    fn ok(&self, cmd: &str, args: Value) -> Value {
        self.call(cmd, args).unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[test]
fn browse_create_rename_and_errors() {
    let h = harness();
    let dir = tempfile::tempdir().unwrap();
    let root = s(dir.path());
    std::fs::write(dir.path().join("b.txt"), "hello fivefiles").unwrap();

    // Empty path on "local" opens Home.
    let home = h.ok("list_dir", json!({ "fs": "local", "path": null }));
    assert_eq!(home["path"].as_str().unwrap(), s(&dirs::home_dir().unwrap()));

    let folder = h.ok("create_dir", json!({ "fs": "local", "parent": root, "name": "Docs" }));
    assert_eq!(folder["isDir"], true);
    assert_eq!(folder["name"], "Docs");
    let file = h.ok("create_file", json!({ "fs": "local", "parent": root, "name": "new.md" }));
    assert_eq!(file["size"], 0);
    assert_eq!(file["ext"], "md");

    let listing = h.ok("list_dir", json!({ "fs": "local", "path": root }));
    let names: Vec<&str> = listing["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    for n in ["Docs", "new.md", "b.txt"] {
        assert!(names.contains(&n), "{n} missing in {names:?}");
    }
    assert_eq!(listing["caps"]["trash"], true);
    assert!(listing["crumbs"].as_array().unwrap().len() >= 2);
    assert_eq!(listing["crumbs"].as_array().unwrap().last().unwrap()["path"], root);

    let renamed = h.ok("rename", json!({ "fs": "local", "path": file["path"], "newName": "notes.md" }));
    assert_eq!(renamed["name"], "notes.md");

    // Typed errors reach the UI as { kind, message }.
    let err = h.call("create_dir", json!({ "fs": "local", "parent": root, "name": "Docs" })).unwrap_err();
    assert_eq!(err["kind"], "alreadyExists");
    let err = h.call("create_dir", json!({ "fs": "local", "parent": root, "name": "a/b" })).unwrap_err();
    assert_eq!(err["kind"], "invalid");
    let err = h.call("list_dir", json!({ "fs": "local", "path": s(&dir.path().join("nope")) })).unwrap_err();
    assert_eq!(err["kind"], "notFound");
    let err = h.call("list_dir", json!({ "fs": "no-such-connection", "path": "/" })).unwrap_err();
    assert_eq!(err["kind"], "notFound");

    let text = h.ok("read_text", json!({ "fs": "local", "path": s(&dir.path().join("b.txt")), "maxBytes": 5 }));
    assert_eq!(text["text"], "hello");
    assert_eq!(text["truncated"], true);
    assert_eq!(text["binary"], false);

    let size = h.ok("dir_size", json!({ "fs": "local", "path": root }));
    assert_eq!(size["files"], 2);
    assert_eq!(size["dirs"], 1);

    let conflicts = h.ok(
        "check_conflicts",
        json!({ "sources": [{ "fs": "local", "path": s(&dir.path().join("b.txt")) }], "dest": { "fs": "local", "path": s(&dir.path().join("Docs")) } }),
    );
    assert_eq!(conflicts, json!([]));
}

#[test]
fn transfer_queue_over_ipc() {
    let h = harness();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/data.bin"), vec![7u8; 3 * 1024 * 1024]).unwrap();
    std::fs::create_dir(dir.path().join("dst")).unwrap();

    let id = h.ok(
        "transfer_enqueue",
        json!({ "req": {
            "kind": "copy",
            "sources": [{ "fs": "local", "path": s(&dir.path().join("src")) }],
            "dest": { "fs": "local", "path": s(&dir.path().join("dst")) },
            "conflict": "keepBoth"
        } }),
    );
    let id = id.as_str().unwrap().to_string();
    let deadline = Instant::now() + Duration::from_secs(20);
    let job = loop {
        let jobs = h.ok("transfers_list", json!({}));
        let job = jobs.as_array().unwrap().iter().find(|j| j["id"] == id.as_str()).cloned().unwrap();
        if ["done", "failed", "cancelled"].contains(&job["status"].as_str().unwrap()) {
            break job;
        }
        assert!(Instant::now() < deadline, "transfer did not finish: {job}");
        std::thread::sleep(Duration::from_millis(30));
    };
    assert_eq!(job["status"], "done", "{job}");
    assert_eq!(job["totalBytes"], 3 * 1024 * 1024);
    assert_eq!(job["doneBytes"], job["totalBytes"]);
    assert_eq!(std::fs::metadata(dir.path().join("dst/src/data.bin")).unwrap().len(), 3 * 1024 * 1024);

    // Same copy now conflicts.
    let conflicts = h.ok(
        "check_conflicts",
        json!({ "sources": [{ "fs": "local", "path": s(&dir.path().join("src")) }], "dest": { "fs": "local", "path": s(&dir.path().join("dst")) } }),
    );
    assert_eq!(conflicts, json!(["src"]));

    h.ok("transfers_clear", json!({}));
    assert_eq!(h.ok("transfers_list", json!({})), json!([]));
}

#[test]
fn connections_are_saved_without_secrets_and_tested() {
    let h = harness();
    let saved = h.ok(
        "connection_save",
        json!({ "input": { "name": "Box", "kind": "ftp", "options": { "host": "127.0.0.1", "port": "1", "password": "pw-secret" }, "color": "#10b981" } }),
    );
    assert_eq!(saved["name"], "Box");
    assert_eq!(saved["status"], "disconnected");
    assert_eq!(saved["storedSecrets"], json!(["password"]));
    assert!(saved["options"].get("password").is_none(), "secret leaked into options: {saved}");

    let list = h.ok("connections_list", json!({}));
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Port 1 refuses connections: test/connect report an error instead of hanging.
    let err = h.call(
        "connection_test",
        json!({ "input": { "id": saved["id"], "name": "Box", "kind": "ftp", "options": { "host": "127.0.0.1", "port": "1" } } }),
    );
    assert!(err.is_err());
    let err = h.call("connection_connect", json!({ "id": saved["id"] })).unwrap_err();
    assert!(!err["message"].as_str().unwrap().is_empty());
    let after = h.ok("connections_list", json!({}));
    assert_eq!(after[0]["status"], "error");

    h.ok("connection_delete", json!({ "id": saved["id"] }));
    assert_eq!(h.ok("connections_list", json!({})), json!([]));
}

#[test]
fn platform_volumes_and_places() {
    let h = harness();
    let p = h.ok("platform", json!({}));
    assert_eq!(p["os"], std::env::consts::OS);
    assert!(!h.ok("get_volumes", json!({})).as_array().unwrap().is_empty());
    assert!(h.ok("get_places", json!({})).as_array().unwrap().iter().any(|p| p["id"] == "home"));
}

#[test]
fn os_drop_uploads_stream_raw_chunks() {
    let h = harness();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("photo.jpg"), b"existing").unwrap();
    let id = h.ok("upload_begin", json!({ "fs": "local", "dir": s(dir.path()), "name": "photo.jpg" }));
    let id = id.as_str().unwrap().to_string();
    for chunk in [vec![1u8; 300_000], vec![2u8; 123]] {
        h.raw("upload_chunk", chunk, &[("x-upload-id", &id)]).unwrap();
    }
    let path = h.ok("upload_end", json!({ "id": id, "cancel": false }));
    // Never overwrites: the dropped file got a fresh name.
    assert!(path.as_str().unwrap().ends_with("photo (2).jpg"), "{path}");
    assert_eq!(std::fs::metadata(path.as_str().unwrap()).unwrap().len(), 300_123);
    assert_eq!(std::fs::read(dir.path().join("photo.jpg")).unwrap(), b"existing");

    // Cancelled uploads leave nothing behind; hostile names are refused.
    let id = h.ok("upload_begin", json!({ "fs": "local", "dir": s(dir.path()), "name": "half.bin" }));
    h.raw("upload_chunk", vec![0u8; 10], &[("x-upload-id", id.as_str().unwrap())]).unwrap();
    assert!(h.call("upload_end", json!({ "id": id, "cancel": true })).is_err());
    assert!(!dir.path().join("half.bin").exists());
    assert_eq!(h.call("upload_begin", json!({ "fs": "local", "dir": s(dir.path()), "name": "../x" })).unwrap_err()["kind"], "invalid");
}
