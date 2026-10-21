//! Remote connections ("integrations"): persisted configuration, secret storage in the
//! OS keychain, and the factory that turns a config into a mounted `Provider`.

use crate::error::{AppError, AppResult};
use crate::vfs::{self, DynProvider};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const KEYRING_SERVICE: &str = "dev.fivefiles.app";
/// Upper bound for connecting + listing the start folder.
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

pub type Opts = HashMap<String, String>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    S3,
    Gdrive,
    Nextcloud,
    Webdav,
    Sftp,
    Ftp,
}

impl Kind {
    /// Option keys that are secrets and therefore never written to the config file.
    pub fn secret_keys(&self) -> &'static [&'static str] {
        match self {
            Kind::S3 => &["secretAccessKey", "sessionToken"],
            Kind::Gdrive => &["clientSecret", "refreshToken", "accessToken"],
            Kind::Nextcloud | Kind::Webdav | Kind::Ftp => &["password"],
            Kind::Sftp => &["password", "passphrase", "privateKey"],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Non-secret options (host, bucket, username…).
    pub options: Opts,
    /// Accent color for the drive icon in the sidebar.
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    /// Secrets are in a plain file instead of the OS keychain (keychain unavailable).
    #[serde(default)]
    pub insecure_secrets: bool,
}

/// What the UI sends when creating/updating: options and secrets mixed; we split them.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInput {
    pub id: Option<String>,
    pub name: String,
    pub kind: Kind,
    pub options: Opts,
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionView {
    #[serde(flatten)]
    pub conn: Connection,
    pub status: Status,
    pub error: Option<String>,
    /// Secret keys that have a stored value (values themselves are never sent to the UI).
    pub stored_secrets: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

pub fn split_secrets(kind: &Kind, all: Opts) -> (Opts, Opts) {
    let secret_keys = kind.secret_keys();
    let (mut opts, mut secrets) = (Opts::new(), Opts::new());
    for (k, v) in all {
        if secret_keys.contains(&k.as_str()) {
            if !v.is_empty() {
                secrets.insert(k, v);
            }
        } else {
            opts.insert(k, v);
        }
    }
    (opts, secrets)
}

// ---------------------------------------------------------------------------------
// Secret storage
// ---------------------------------------------------------------------------------

pub struct SecretStore {
    fallback_file: PathBuf,
    use_keychain: bool,
}

/// Windows Credential Manager caps a secret at 2560 bytes (UTF-16), which a pasted RSA
/// key exceeds; the JSON is therefore split across numbered entries everywhere.
const CHUNK_CHARS: usize = 1000;

/// Keychain backends may block on D-Bus (Linux Secret Service drives its own runtime and
/// panics inside a Tokio worker), so every call runs on a short-lived plain thread.
fn off_runtime<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| s.spawn(f).join().expect("keychain thread panicked"))
}

impl SecretStore {
    pub fn new(dir: &std::path::Path, use_keychain: bool) -> Self {
        Self { fallback_file: dir.join("secrets.json"), use_keychain }
    }

    fn entry(name: &str) -> Option<keyring::Entry> {
        keyring::Entry::new(KEYRING_SERVICE, name).ok()
    }

    fn keychain_write(id: &str, json: &str) -> bool {
        let chars: Vec<char> = json.chars().collect();
        let chunks: Vec<String> = chars.chunks(CHUNK_CHARS).map(|c| c.iter().collect()).collect();
        let Some(head) = Self::entry(id) else { return false };
        for (i, c) in chunks.iter().enumerate() {
            match Self::entry(&format!("{id}#{i}")) {
                Some(e) if e.set_password(c).is_ok() => {}
                _ => return false,
            }
        }
        head.set_password(&format!("chunks:{}", chunks.len())).is_ok()
    }

    fn keychain_read(id: &str) -> Option<String> {
        let head = Self::entry(id)?.get_password().ok()?;
        let n: usize = head.strip_prefix("chunks:")?.parse().ok()?;
        let mut out = String::new();
        for i in 0..n {
            out.push_str(&Self::entry(&format!("{id}#{i}"))?.get_password().ok()?);
        }
        Some(out)
    }

    fn keychain_delete(id: &str) {
        let n = Self::entry(id)
            .and_then(|e| e.get_password().ok())
            .and_then(|h| h.strip_prefix("chunks:").and_then(|n| n.parse::<usize>().ok()))
            .unwrap_or(0);
        for i in 0..n {
            if let Some(e) = Self::entry(&format!("{id}#{i}")) {
                let _ = e.delete_credential();
            }
        }
        if let Some(e) = Self::entry(id) {
            let _ = e.delete_credential();
        }
    }

    fn read_fallback(&self) -> HashMap<String, Opts> {
        std::fs::read(&self.fallback_file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn write_fallback(&self, all: &HashMap<String, Opts>) -> AppResult<()> {
        let data = serde_json::to_vec_pretty(all)?;
        let tmp = self.fallback_file.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            // Owner-only from the first byte (no world-readable window).
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            f.write_all(&data)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.fallback_file)?;
        Ok(())
    }

    /// Returns `true` if stored in the OS keychain, `false` if in the fallback file.
    pub fn save(&self, id: &str, secrets: &Opts) -> AppResult<bool> {
        let json = serde_json::to_string(secrets)?;
        let keychain_ok = self.use_keychain && off_runtime(|| Self::keychain_write(id, &json));
        let mut fb = self.read_fallback();
        if keychain_ok {
            if fb.remove(id).is_some() {
                self.write_fallback(&fb)?;
            }
            Ok(true)
        } else {
            fb.insert(id.into(), secrets.clone());
            self.write_fallback(&fb)?;
            Ok(false)
        }
    }

    pub fn load(&self, id: &str) -> Opts {
        if self.use_keychain
            && let Some(json) = off_runtime(|| Self::keychain_read(id))
            && let Ok(v) = serde_json::from_str(&json)
        {
            return v;
        }
        self.read_fallback().remove(id).unwrap_or_default()
    }

    pub fn delete(&self, id: &str) {
        if self.use_keychain {
            off_runtime(|| Self::keychain_delete(id));
        }
        let mut fb = self.read_fallback();
        if fb.remove(id).is_some() {
            let _ = self.write_fallback(&fb);
        }
    }
}

// ---------------------------------------------------------------------------------
// Connection store + mounted providers
// ---------------------------------------------------------------------------------

pub struct Connections {
    file: PathBuf,
    secrets: SecretStore,
    list: RwLock<Vec<Connection>>,
    mounted: RwLock<HashMap<String, DynProvider>>,
    state: RwLock<HashMap<String, (Status, Option<String>)>>,
    /// One lock per connection: concurrent first uses share a single connect, while an
    /// unreachable host doesn't hold up the others.
    connect_locks: parking_lot::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Connections {
    pub fn load(dir: PathBuf) -> Self {
        Self::load_with(dir, true)
    }

    pub fn load_with(dir: PathBuf, use_keychain: bool) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("connections.json");
        let list = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Self {
            secrets: SecretStore::new(&dir, use_keychain),
            file,
            list: RwLock::new(list),
            mounted: RwLock::new(HashMap::new()),
            state: RwLock::new(HashMap::new()),
            connect_locks: Default::default(),
        }
    }

    fn persist(&self) -> AppResult<()> {
        let data = serde_json::to_vec_pretty(&*self.list.read())?;
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, &self.file)?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<Connection> {
        self.list.read().iter().find(|c| c.id == id).cloned()
    }

    pub fn views(&self) -> Vec<ConnectionView> {
        let state = self.state.read();
        self.list
            .read()
            .iter()
            .map(|c| {
                let (status, error) = state.get(&c.id).cloned().unwrap_or((Status::Disconnected, None));
                let stored_secrets = self.secrets.load(&c.id).into_keys().collect();
                ConnectionView { conn: c.clone(), status, error, stored_secrets }
            })
            .collect()
    }

    /// Creates or updates a connection. Blank secret fields keep the stored value.
    pub fn upsert(&self, input: ConnectionInput) -> AppResult<Connection> {
        if input.name.trim().is_empty() {
            return Err(AppError::Invalid("name is required".into()));
        }
        let (mut opts, new_secrets) = split_secrets(&input.kind, input.options);
        let id = input.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut secrets = self.secrets.load(&id);
        secrets.extend(new_secrets);
        let in_keychain = self.secrets.save(&id, &secrets)?;
        let existing = self.get(&id);
        // Trusted SSH host key: it is learned by the backend after the form was opened,
        // so an absent field means "keep" (an explicit empty value clears it), and it is
        // always dropped when the connection now points at another host:port.
        if let Some(prev) = &existing {
            match opts.get("hostKey").map(|v| v.trim().to_string()) {
                None => {
                    if let Some(k) = prev.options.get("hostKey") {
                        opts.insert("hostKey".into(), k.clone());
                    }
                }
                Some(v) if v.is_empty() => {
                    opts.remove("hostKey");
                }
                Some(_) => {}
            }
            let moved = ["host", "port"].iter().any(|k| prev.options.get(*k) != opts.get(*k));
            if moved {
                opts.remove("hostKey");
            }
        }
        let conn = Connection {
            id: id.clone(),
            name: input.name.trim().to_string(),
            kind: input.kind,
            options: opts,
            color: input.color,
            created_at: existing.as_ref().map(|c| c.created_at).unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
            insecure_secrets: !in_keychain,
        };
        {
            let mut list = self.list.write();
            match list.iter_mut().find(|c| c.id == id) {
                Some(c) => *c = conn.clone(),
                None => list.push(conn.clone()),
            }
        }
        self.persist()?;
        // Config changed: drop the live session so the next access reconnects.
        self.disconnect(&id);
        Ok(conn)
    }

    pub fn remove(&self, id: &str) -> AppResult<()> {
        self.disconnect(id);
        self.list.write().retain(|c| c.id != id);
        self.secrets.delete(id);
        self.state.write().remove(id);
        self.persist()
    }

    pub fn disconnect(&self, id: &str) {
        self.mounted.write().remove(id);
        self.state.write().insert(id.into(), (Status::Disconnected, None));
    }

    fn set_option(&self, id: &str, key: &str, value: String) -> AppResult<()> {
        if let Some(c) = self.list.write().iter_mut().find(|c| c.id == id) {
            c.options.insert(key.into(), value);
        }
        self.persist()
    }

    pub fn secrets_for(&self, id: &str) -> Opts {
        self.secrets.load(id)
    }

    pub fn set_secret(&self, id: &str, key: &str, value: String) -> AppResult<()> {
        let mut s = self.secrets.load(id);
        s.insert(key.into(), value);
        self.secrets.save(id, &s)?;
        Ok(())
    }

    /// Returns the mounted provider, connecting on first use.
    pub async fn provider(&self, id: &str) -> AppResult<DynProvider> {
        if let Some(p) = self.mounted.read().get(id).cloned() {
            return Ok(p);
        }
        let lock = self.connect_locks.lock().entry(id.to_string()).or_default().clone();
        let _guard = lock.lock().await;
        if let Some(p) = self.mounted.read().get(id).cloned() {
            return Ok(p);
        }
        let conn = self.get(id).ok_or_else(|| AppError::NotFound(format!("connection {id}")))?;
        self.state.write().insert(id.into(), (Status::Connecting, None));
        let secrets = self.secrets.load(id);
        let attempt = async {
            let built = build(&conn, &secrets).await?;
            // Probe so bad credentials / unreachable hosts surface on connect.
            built.provider.list(&built.provider.root()).await?;
            Ok::<_, AppError>(built)
        };
        let result = match tokio::time::timeout(CONNECT_TIMEOUT, attempt).await {
            Ok(r) => r,
            Err(_) => Err(AppError::Connection(format!("no answer after {}s — check the host and port", CONNECT_TIMEOUT.as_secs()))),
        };
        match result {
            Ok(built) => {
                if let Some(fp) = built.learned_host_key {
                    self.set_option(id, "hostKey", fp)?;
                }
                self.mounted.write().insert(id.into(), built.provider.clone());
                self.state.write().insert(id.into(), (Status::Connected, None));
                Ok(built.provider)
            }
            Err(e) => {
                self.state.write().insert(id.into(), (Status::Error, Some(e.to_string())));
                Err(e)
            }
        }
    }

    pub fn status(&self, id: &str) -> Status {
        self.state.read().get(id).map(|s| s.0).unwrap_or(Status::Disconnected)
    }
}

pub struct Built {
    pub provider: DynProvider,
    /// SSH host key fingerprint accepted on first connect (trust-on-first-use).
    pub learned_host_key: Option<String>,
}

fn opt<'a>(o: &'a Opts, k: &str) -> Option<&'a str> {
    o.get(k).map(|s| s.trim()).filter(|s| !s.is_empty())
}

fn req<'a>(o: &'a Opts, k: &str) -> AppResult<&'a str> {
    opt(o, k).ok_or_else(|| AppError::Invalid(format!("missing required field: {k}")))
}

pub async fn build(conn: &Connection, secrets: &Opts) -> AppResult<Built> {
    let o = &conn.options;
    let s = secrets;
    let root = opt(o, "root").unwrap_or("/");
    let dal = |op: opendal::Operator| -> AppResult<Built> {
        Ok(Built { provider: Arc::new(vfs::dal::DalFs::new(op, true)), learned_host_key: None })
    };
    match conn.kind {
        Kind::S3 => {
            let mut b =
                opendal::services::S3::default().bucket(req(o, "bucket")?).root(root).region(opt(o, "region").unwrap_or("us-east-1"));
            if let Some(ep) = opt(o, "endpoint") {
                b = b.endpoint(ep);
            }
            if let Some(k) = opt(o, "accessKeyId") {
                b = b.access_key_id(k);
            }
            if let Some(k) = opt(s, "secretAccessKey") {
                b = b.secret_access_key(k);
            }
            if let Some(t) = opt(s, "sessionToken") {
                b = b.session_token(t);
            }
            if opt(o, "pathStyle") != Some("true") {
                b = b.enable_virtual_host_style();
            }
            // Never fall back to ambient AWS credentials/IMDS: only what the user typed.
            b = b.disable_config_load().disable_ec2_metadata();
            dal(opendal::Operator::new(b)?.finish())
        }
        Kind::Gdrive => {
            let mut b = opendal::services::Gdrive::default().root(root);
            if let Some(t) = opt(s, "accessToken") {
                b = b.access_token(t);
            }
            if let Some(t) = opt(s, "refreshToken") {
                b = b.refresh_token(t);
            }
            if let Some(c) = opt(o, "clientId") {
                b = b.client_id(c);
            }
            if let Some(c) = opt(s, "clientSecret") {
                b = b.client_secret(c);
            }
            if opt(s, "refreshToken").is_none() && opt(s, "accessToken").is_none() {
                return Err(AppError::Auth("sign in with Google first".into()));
            }
            dal(opendal::Operator::new(b)?.finish())
        }
        Kind::Nextcloud | Kind::Webdav => {
            let user = opt(o, "username");
            let endpoint = if conn.kind == Kind::Nextcloud {
                nextcloud_endpoint(req(o, "url")?, user.ok_or_else(|| AppError::Invalid("missing required field: username".into()))?)
            } else {
                req(o, "url")?.trim_end_matches('/').to_string()
            };
            let mut b = opendal::services::Webdav::default().endpoint(&endpoint).root(root);
            if let Some(u) = user {
                b = b.username(u);
            }
            if let Some(p) = opt(s, "password") {
                b = b.password(p);
            }
            let put = vfs::spool::HttpPut {
                client: reqwest::Client::new(),
                base: format!("{endpoint}{}", vfs::posix::normalize(root).trim_end_matches('/')),
                user: user.map(String::from),
                pass: opt(s, "password").map(String::from),
            };
            let fs = vfs::dal::DalFs::new(opendal::Operator::new(b)?.finish(), true).with_put(put);
            Ok(Built { provider: Arc::new(fs), learned_host_key: None })
        }
        Kind::Ftp => {
            let scheme = if opt(o, "secure") == Some("true") { "ftps" } else { "ftp" };
            let port = opt(o, "port").unwrap_or("21");
            let endpoint = format!("{scheme}://{}:{port}", req(o, "host")?);
            // OpenDAL's FTP pool retries silently; fail fast on refused/unreachable hosts.
            probe_tcp(req(o, "host")?, port).await?;
            let mut b = opendal::services::Ftp::default().endpoint(&endpoint).root(root);
            if let Some(u) = opt(o, "username") {
                b = b.user(u);
            }
            if let Some(p) = opt(s, "password") {
                b = b.password(p);
            }
            let empty = vfs::ftp_empty::FtpTarget {
                addr: format!("{}:{port}", req(o, "host")?),
                secure: scheme == "ftps",
                user: opt(o, "username").unwrap_or_default().to_string(),
                password: opt(s, "password").unwrap_or_default().to_string(),
                root: vfs::posix::normalize(root),
            };
            let fs = vfs::dal::DalFs::new(opendal::Operator::new(b)?.finish(), true).with_ftp_empty(empty);
            Ok(Built { provider: Arc::new(fs), learned_host_key: None })
        }
        Kind::Sftp => {
            let (fs, learned) = vfs::sftp::SftpFs::connect(o, s).await?;
            Ok(Built { provider: Arc::new(fs), learned_host_key: learned })
        }
    }
}

async fn probe_tcp(host: &str, port: &str) -> AppResult<()> {
    let addr = format!("{host}:{port}");
    match tokio::time::timeout(std::time::Duration::from_secs(8), tokio::net::TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(AppError::Connection(format!("can't reach {addr}: {e}"))),
        Err(_) => Err(AppError::Connection(format!("can't reach {addr}: timed out"))),
    }
}

/// `https://cloud.example.com` + `ana` → `https://cloud.example.com/remote.php/dav/files/ana`
pub fn nextcloud_endpoint(url: &str, user: &str) -> String {
    let base = url.trim().trim_end_matches('/');
    let base = if base.starts_with("http://") || base.starts_with("https://") { base.to_string() } else { format!("https://{base}") };
    if base.contains("/remote.php/") {
        return base;
    }
    format!("{base}/remote.php/dav/files/{}", urlencoding::encode(user))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_split_from_options() {
        let mut all = Opts::new();
        all.insert("host".into(), "example.com".into());
        all.insert("password".into(), "hunter2".into());
        all.insert("passphrase".into(), "".into());
        let (o, s) = split_secrets(&Kind::Sftp, all);
        assert_eq!(o.get("host").unwrap(), "example.com");
        assert!(!o.contains_key("password"));
        assert_eq!(s.get("password").unwrap(), "hunter2");
        // Empty secrets are dropped so they don't overwrite stored values.
        assert!(!s.contains_key("passphrase"));
    }

    #[test]
    fn changing_host_forgets_the_trusted_host_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = Connections::load_with(dir.path().to_path_buf(), false);
        let mut o = Opts::new();
        o.insert("host".into(), "a.example.com".into());
        o.insert("hostKey".into(), "SHA256:abc".into());
        let c = store.upsert(ConnectionInput { id: None, name: "s".into(), kind: Kind::Sftp, options: o.clone(), color: None }).unwrap();
        // Same host: kept.
        let c =
            store.upsert(ConnectionInput { id: Some(c.id), name: "s".into(), kind: Kind::Sftp, options: o.clone(), color: None }).unwrap();
        assert_eq!(c.options.get("hostKey").map(String::as_str), Some("SHA256:abc"));
        // Form opened before the key was learned (field absent): kept.
        let mut stale = o.clone();
        stale.remove("hostKey");
        let c = store
            .upsert(ConnectionInput { id: Some(c.id), name: "renamed".into(), kind: Kind::Sftp, options: stale, color: None })
            .unwrap();
        assert_eq!(c.options.get("hostKey").map(String::as_str), Some("SHA256:abc"));
        // New host: dropped.
        o.insert("host".into(), "b.example.com".into());
        let c = store.upsert(ConnectionInput { id: Some(c.id), name: "s".into(), kind: Kind::Sftp, options: o, color: None }).unwrap();
        assert!(c.options.get("hostKey").is_none());
    }

    /// Touches the real OS keychain: `cargo test keychain -- --ignored`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn keychain_roundtrip_with_large_secret_inside_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::new(dir.path(), true);
        let id = format!("5files-test-{}", uuid::Uuid::new_v4());
        let mut secrets = Opts::new();
        secrets.insert("privateKey".into(), "k".repeat(3300));
        secrets.insert("passphrase".into(), "pässwörd".into());
        assert!(store.save(&id, &secrets).unwrap(), "should land in the keychain, not the fallback file");
        assert_eq!(store.load(&id), secrets);
        assert!(!dir.path().join("secrets.json").exists());
        store.delete(&id);
        assert!(store.load(&id).is_empty());
    }

    #[test]
    fn nextcloud_url_building() {
        assert_eq!(nextcloud_endpoint("cloud.x.com/", "ana"), "https://cloud.x.com/remote.php/dav/files/ana");
        assert_eq!(nextcloud_endpoint("http://h:8080", "a b"), "http://h:8080/remote.php/dav/files/a%20b");
        assert_eq!(nextcloud_endpoint("https://h/remote.php/dav/files/x", "ignored"), "https://h/remote.php/dav/files/x");
    }

    #[test]
    fn store_roundtrip_without_leaking_secrets_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = Connections::load_with(dir.path().to_path_buf(), false);
        let mut options = Opts::new();
        options.insert("host".into(), "ftp.example.com".into());
        options.insert("password".into(), "s3cr3t-value".into());
        let c = store.upsert(ConnectionInput { id: None, name: "My FTP".into(), kind: Kind::Ftp, options, color: None }).unwrap();
        let on_disk = std::fs::read_to_string(dir.path().join("connections.json")).unwrap();
        assert!(on_disk.contains("ftp.example.com"));
        assert!(!on_disk.contains("s3cr3t-value"));

        // Reload from disk.
        let store2 = Connections::load_with(dir.path().to_path_buf(), false);
        let views = store2.views();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].conn.name, "My FTP");
        assert_eq!(store2.secrets.load(&c.id).get("password").map(String::as_str), Some("s3cr3t-value"));

        store2.remove(&c.id).unwrap();
        assert!(store2.views().is_empty());
        assert!(store2.secrets.load(&c.id).is_empty());
    }
}
