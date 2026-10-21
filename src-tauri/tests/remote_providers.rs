//! End-to-end tests of the remote providers against real servers in Docker.
//!
//! ```sh
//! tests/docker/up.sh
//! cargo test --test remote_providers -- --ignored --test-threads=1
//! tests/docker/down.sh
//! ```
//!
//! Endpoints can be overridden with env vars (see `env_or` calls); the defaults match
//! `tests/docker/up.sh`. Ignored by default so plain `cargo test` needs no Docker.

use atlas_lib::connections::{self, Connection, Kind, Opts};
use atlas_lib::error::AppError;
use atlas_lib::metrics;
use atlas_lib::vfs::DynProvider;
use rand::RngCore;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const FILE_SIZE: usize = 5 * 1024 * 1024 + 12_345; // not a multiple of any chunk size

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

fn test_dir() -> PathBuf {
    match std::env::var("ATLAS_TEST_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => std::env::temp_dir().join("atlas-remote-tests"),
    }
}

fn opts(pairs: &[(&str, String)]) -> Opts {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

fn conn(kind: Kind, options: Opts) -> Connection {
    Connection {
        id: format!("it-{}", uuid::Uuid::new_v4()),
        name: "integration".into(),
        kind,
        options,
        color: None,
        created_at: 0,
        insecure_secrets: true,
    }
}

fn random_bytes(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    rand::rng().fill_bytes(&mut v);
    v
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

async fn write_file(p: &DynProvider, path: &str, data: &[u8]) {
    let mut w = p.writer(path).await.unwrap_or_else(|e| panic!("writer({path}): {e}"));
    // Uneven write sizes exercise buffering.
    for chunk in data.chunks(100_003) {
        w.write_all(chunk).await.unwrap();
    }
    w.shutdown().await.unwrap_or_else(|e| panic!("shutdown({path}): {e}"));
}

async fn read_file(p: &DynProvider, path: &str) -> Vec<u8> {
    let mut r = p.reader(path).await.unwrap_or_else(|e| panic!("reader({path}): {e}"));
    let mut out = Vec::new();
    r.read_to_end(&mut out).await.unwrap();
    out
}

/// Fails the test (exits the process) if it runs longer than `secs`. A tokio timeout
/// isn't enough: a provider that busy-loops inside one poll never yields to it.
struct Watchdog(Option<std::sync::mpsc::Sender<()>>);

fn watchdog(name: &'static str, secs: u64) -> Watchdog {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(std::time::Duration::from_secs(secs)) {
            eprintln!("\n{name}: still running after {secs}s — the provider is hung (busy loop or stalled I/O)");
            std::process::exit(101);
        }
    });
    Watchdog(Some(tx))
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

/// The common scenario every backend must pass. `base` is a writable folder.
async fn exercise(p: DynProvider, base: &str) {
    assert!(p.is_remote());
    let root = p.root();
    p.list(&root).await.unwrap_or_else(|e| panic!("list root {root}: {e}"));

    let top = p.join(base, &unique("atlas-it"));
    let nested = p.join(&p.join(&top, "a"), "b c"); // space in a name, on purpose
    p.mkdir(&nested).await.unwrap_or_else(|e| panic!("mkdir {nested}: {e}"));
    assert!(p.stat(&nested).await.unwrap().is_dir);
    // mkdir is idempotent.
    p.mkdir(&nested).await.unwrap();

    let data = random_bytes(FILE_SIZE);
    let file = p.join(&nested, "blob.bin");
    let before = metrics::totals();
    write_file(&p, &file, &data).await;

    let st = p.stat(&file).await.unwrap();
    assert!(!st.is_dir);
    assert_eq!(st.size, FILE_SIZE as u64, "stat size");
    assert_eq!(st.name, "blob.bin");
    assert_eq!(st.ext.as_deref(), Some("bin"));

    let listed = p.list(&nested).await.unwrap();
    let e = listed.iter().find(|e| e.name == "blob.bin").unwrap_or_else(|| panic!("blob.bin not listed: {listed:?}"));
    assert_eq!(e.size, FILE_SIZE as u64, "listed size");
    assert_eq!(e.path, file);
    assert!(!e.is_dir);
    assert!(listed.iter().all(|e| e.name != "." && e.name != ".."));

    let back = read_file(&p, &file).await;
    assert_eq!(back.len(), data.len(), "read-back length");
    assert!(back == data, "read-back content differs");

    let after = metrics::totals();
    assert!(after.net_tx - before.net_tx >= FILE_SIZE as u64, "net_tx grew by {}", after.net_tx - before.net_tx);
    assert!(after.net_rx - before.net_rx >= FILE_SIZE as u64, "net_rx grew by {}", after.net_rx - before.net_rx);

    // Small files and empty files round-trip too.
    let small = p.join(&nested, ".hidden.txt");
    write_file(&p, &small, b"hello").await;
    assert_eq!(read_file(&p, &small).await, b"hello");
    let small_e = p.list(&nested).await.unwrap().into_iter().find(|e| e.name == ".hidden.txt").unwrap();
    assert!(small_e.hidden);
    let empty = p.join(&nested, "empty");
    write_file(&p, &empty, b"").await;
    assert_eq!(p.stat(&empty).await.unwrap().size, 0);
    assert!(read_file(&p, &empty).await.is_empty());
    // Overwrite truncates.
    write_file(&p, &small, b"hi").await;
    assert_eq!(read_file(&p, &small).await, b"hi");

    if p.caps().rename {
        let renamed = p.join(&nested, "renamed.bin");
        p.rename(&file, &renamed).await.unwrap_or_else(|e| panic!("rename: {e}"));
        assert!(!p.exists(&file).await.unwrap());
        assert_eq!(p.stat(&renamed).await.unwrap().size, FILE_SIZE as u64);
        // Renaming onto an existing entry is refused.
        match p.rename(&renamed, &small).await {
            Err(AppError::AlreadyExists(_)) => {}
            other => panic!("rename onto existing file: expected AlreadyExists, got {other:?}"),
        }
    }

    assert!(matches!(p.stat(&p.join(&top, "nope")).await, Err(AppError::NotFound(_))));

    p.remove(&top).await.unwrap_or_else(|e| panic!("remove {top}: {e}"));
    assert!(!p.exists(&top).await.unwrap(), "{top} still exists");
    assert!(!p.exists(&nested).await.unwrap());
}

// ---------------------------------------------------------------------------------
// SFTP
// ---------------------------------------------------------------------------------

fn sftp_opts() -> Opts {
    opts(&[
        ("host", env_or("ATLAS_SFTP_HOST", "127.0.0.1")),
        ("port", env_or("ATLAS_SFTP_PORT", "22222")),
        ("username", env_or("ATLAS_SFTP_USER", "atlas")),
    ])
}

fn sftp_password() -> Opts {
    opts(&[("password", env_or("ATLAS_SFTP_PASS", "atlaspass"))])
}

/// atmoz/sftp chroots the user into a root-owned home; `upload/` is the writable folder.
fn sftp_base() -> String {
    env_or("ATLAS_SFTP_BASE", "/upload")
}

#[tokio::test]
#[ignore = "needs docker: tests/docker/up.sh"]
async fn sftp_password_roundtrip() {
    let _wd = watchdog("sftp_password_roundtrip", 120);
    let c = conn(Kind::Sftp, sftp_opts());
    let built = connections::build(&c, &sftp_password()).await.unwrap();
    assert!(built.learned_host_key.is_some());
    let p = built.provider;
    assert!(p.root().starts_with('/'));
    assert!(!p.caps().server_copy && !p.caps().trash && p.caps().rename);
    exercise(p, &sftp_base()).await;
}

#[tokio::test]
#[ignore = "needs docker: tests/docker/up.sh"]
async fn sftp_key_auth() {
    let _wd = watchdog("sftp_key_auth", 120);
    let key = env_or("ATLAS_SFTP_KEY", &test_dir().join("id_ed25519").to_string_lossy());
    // From a key file…
    let mut o = sftp_opts();
    o.insert("authMethod".into(), "key".into());
    o.insert("keyPath".into(), key.clone());
    let p = connections::build(&conn(Kind::Sftp, o.clone()), &Opts::new()).await.unwrap().provider;
    p.list(&p.root()).await.unwrap();
    // …and from pasted key content (no authMethod: inferred from the key).
    let pem = std::fs::read_to_string(&key).unwrap();
    let mut o2 = sftp_opts();
    o2.remove("authMethod");
    let p2 = connections::build(&conn(Kind::Sftp, o2), &opts(&[("privateKey", pem)])).await.unwrap().provider;
    p2.list(&p2.root()).await.unwrap();
}

#[tokio::test]
#[ignore = "needs docker: tests/docker/up.sh"]
async fn sftp_bad_credentials_are_auth_errors() {
    let _wd = watchdog("sftp_bad_credentials_are_auth_errors", 120);
    let r = connections::build(&conn(Kind::Sftp, sftp_opts()), &opts(&[("password", "wrong".into())])).await;
    assert!(matches!(r, Err(AppError::Auth(_))), "{:?}", r.err());

    // A key the server doesn't know.
    let dir = tempfile::tempdir().unwrap();
    let kp = dir.path().join("k");
    let st = std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&kp).status().unwrap();
    assert!(st.success());
    let mut o = sftp_opts();
    o.insert("keyPath".into(), kp.to_string_lossy().into_owned());
    let r = connections::build(&conn(Kind::Sftp, o), &Opts::new()).await;
    assert!(matches!(r, Err(AppError::Auth(_))), "{:?}", r.err());
}

#[tokio::test]
#[ignore = "needs docker: tests/docker/up.sh"]
async fn sftp_unreachable_is_connection_error() {
    let _wd = watchdog("sftp_unreachable_is_connection_error", 120);
    let mut o = sftp_opts();
    o.insert("port".into(), "1".into());
    let r = connections::build(&conn(Kind::Sftp, o), &sftp_password()).await;
    assert!(matches!(r, Err(AppError::Connection(_))), "{:?}", r.err());
}

#[tokio::test]
#[ignore = "needs docker: tests/docker/up.sh"]
async fn sftp_host_key_tofu() {
    let _wd = watchdog("sftp_host_key_tofu", 120);
    let first = connections::build(&conn(Kind::Sftp, sftp_opts()), &sftp_password()).await.unwrap();
    let fp = first.learned_host_key.clone().expect("first connect learns the host key");
    assert!(fp.starts_with("SHA256:"), "{fp}");

    let mut trusted = sftp_opts();
    trusted.insert("hostKey".into(), fp.clone());
    let again = connections::build(&conn(Kind::Sftp, trusted), &sftp_password()).await.unwrap();
    assert!(again.learned_host_key.is_none(), "known key must not be re-learned");
    again.provider.list(&again.provider.root()).await.unwrap();

    let mut wrong = sftp_opts();
    wrong.insert("hostKey".into(), "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into());
    match connections::build(&conn(Kind::Sftp, wrong), &sftp_password()).await {
        Err(AppError::Auth(m)) => assert!(m.contains("host key mismatch"), "{m}"),
        Err(e) => panic!("expected Auth error, got {e:?}"),
        Ok(_) => panic!("connected despite host key mismatch"),
    }
}

#[tokio::test]
#[ignore = "needs docker: tests/docker/up.sh"]
async fn sftp_concurrent_transfers_share_one_session() {
    let _wd = watchdog("sftp_concurrent_transfers_share_one_session", 120);
    let p = connections::build(&conn(Kind::Sftp, sftp_opts()), &sftp_password()).await.unwrap().provider;
    let top = p.join(&sftp_base(), &unique("atlas-conc"));
    p.mkdir(&top).await.unwrap();
    let mut tasks = Vec::new();
    for i in 0..3 {
        let p = p.clone();
        let path = p.join(&top, &format!("f{i}.bin"));
        tasks.push(tokio::spawn(async move {
            let data = random_bytes(2 * 1024 * 1024 + i * 777);
            write_file(&p, &path, &data).await;
            assert!(read_file(&p, &path).await == data, "content mismatch for {path}");
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(p.list(&top).await.unwrap().len(), 3);
    p.remove(&top).await.unwrap();
    assert!(!p.exists(&top).await.unwrap());
}

#[tokio::test]
#[ignore = "needs docker: tests/docker/up.sh"]
async fn sftp_symlinks_are_followed_for_type() {
    let _wd = watchdog("sftp_symlinks_are_followed_for_type", 120);
    let p = connections::build(&conn(Kind::Sftp, sftp_opts()), &sftp_password()).await.unwrap().provider;
    let top = p.join(&sftp_base(), &unique("atlas-link"));
    let target = p.join(&top, "dir");
    p.mkdir(&target).await.unwrap();
    write_file(&p, &p.join(&target, "x.txt"), b"x").await;
    // No symlink API on the trait: create one with the container's shell.
    let link_in_container = format!("/home/atlas{}", p.join(&top, "link"));
    let st = std::process::Command::new("docker").args(["exec", "atlas-test-sftp", "ln", "-s", "dir", &link_in_container]).status();
    if !matches!(st, Ok(s) if s.success()) {
        eprintln!("skipping: cannot create a symlink inside the container");
        p.remove(&top).await.unwrap();
        return;
    }
    let entries = p.list(&top).await.unwrap();
    let link = entries.iter().find(|e| e.name == "link").expect("link listed");
    assert!(link.is_symlink && link.is_dir, "{link:?}");
    // Removing the tree removes the link, not through it twice.
    p.remove(&top).await.unwrap();
    assert!(!p.exists(&top).await.unwrap());
}

// ---------------------------------------------------------------------------------
// FTP / S3 / WebDAV (OpenDAL-backed)
// ---------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs docker: tests/docker/up.sh"]
async fn ftp_roundtrip() {
    let _wd = watchdog("ftp_roundtrip", 120);
    let c = conn(
        Kind::Ftp,
        opts(&[
            ("host", env_or("ATLAS_FTP_HOST", "127.0.0.1")),
            ("port", env_or("ATLAS_FTP_PORT", "22121")),
            ("username", env_or("ATLAS_FTP_USER", "atlas")),
            // delfer/alpine-ftp-server doesn't chroot users; scope the connection to home.
            ("root", env_or("ATLAS_FTP_ROOT", "/ftp/atlas")),
        ]),
    );
    let secrets = opts(&[("password", env_or("ATLAS_FTP_PASS", "atlaspass"))]);
    let p = connections::build(&c, &secrets).await.unwrap().provider;
    exercise(p, "/").await;
}

#[tokio::test]
#[ignore = "needs docker: tests/docker/up.sh"]
async fn s3_roundtrip() {
    let _wd = watchdog("s3_roundtrip", 120);
    let c = conn(
        Kind::S3,
        opts(&[
            ("bucket", env_or("ATLAS_S3_BUCKET", "atlas-test")),
            ("endpoint", env_or("ATLAS_S3_ENDPOINT", "http://127.0.0.1:29000")),
            ("region", env_or("ATLAS_S3_REGION", "us-east-1")),
            ("accessKeyId", env_or("ATLAS_S3_ACCESS_KEY", "atlas")),
            ("pathStyle", "true".into()),
        ]),
    );
    let secrets = opts(&[("secretAccessKey", env_or("ATLAS_S3_SECRET_KEY", "atlassecret"))]);
    let p = connections::build(&c, &secrets).await.unwrap().provider;
    exercise(p, "/").await;
}

#[tokio::test]
#[ignore = "needs docker: tests/docker/up.sh"]
async fn webdav_roundtrip() {
    let _wd = watchdog("webdav_roundtrip", 120);
    let c = conn(
        Kind::Webdav,
        opts(&[("url", env_or("ATLAS_WEBDAV_URL", "http://127.0.0.1:28080")), ("username", env_or("ATLAS_WEBDAV_USER", "atlas"))]),
    );
    let secrets = opts(&[("password", env_or("ATLAS_WEBDAV_PASS", "atlaspass"))]);
    let p = connections::build(&c, &secrets).await.unwrap().provider;
    exercise(p, "/").await;
}
