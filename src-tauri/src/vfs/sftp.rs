//! SFTP provider (pure Rust, via russh + russh-sftp) — works on macOS, Windows and Linux.
//!
//! One SSH connection + one SFTP channel per mounted connection, shared (`Arc`) by every
//! task. russh-sftp multiplexes requests by id, so concurrent operations (the transfer
//! queue runs several jobs at once) are safe and pipelined over the same channel.
//!
//! Throughput notes:
//! - Writes: russh-sftp's `File` already pipelines up to `max_concurrent_writes`
//!   unacknowledged WRITE requests; we feed it full-size chunks through a `BufWriter`.
//! - Reads: `File` issues one READ at a time, so a single handle is bound by RTT. The
//!   reader opens several handles on the same file and fetches interleaved chunks in
//!   parallel, reassembling them in order.

use super::{BoxReader, BoxWriter, Caps, Entry, Provider, Usage, posix};
use crate::connections::Opts;
use crate::error::{AppError, AppResult};
use crate::metrics::{self, Channel, CountingReader, CountingWriter};
use async_trait::async_trait;
use futures::StreamExt;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use russh::client;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::fs::{File, Metadata};
use russh_sftp::protocol::StatusCode;
use std::io::{self, SeekFrom};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufWriter, ReadBuf};
use tokio::sync::mpsc;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const AUTH_TIMEOUT: Duration = Duration::from_secs(30);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);
const KEEPALIVE_MAX: usize = 3;
/// Per-request SFTP response timeout. Generous because a full-size READ on a slow link
/// can legitimately take a while.
const REQUEST_TIMEOUT_SECS: u64 = 60;
/// Matches OpenSSH's advertised max read/write length (256 KiB - 1 KiB) so each buffered
/// flush maps to exactly one WRITE request.
const IO_CHUNK: usize = 255 * 1024;
/// Bytes fetched per parallel read task before moving to its next interleaved chunk.
const READ_CHUNK: usize = 4 * IO_CHUNK;
/// Parallel handles used for one download.
const READ_STREAMS: usize = 4;
const MAX_CONCURRENT_WRITES: usize = 16;
/// Rough wire cost of one stat/list entry, used to account metadata traffic in metrics.
const META_WIRE_BYTES: u64 = 256;

// ---------------------------------------------------------------------------------
// SSH client handler: host-key trust-on-first-use
// ---------------------------------------------------------------------------------

#[derive(Debug)]
enum SshErr {
    Ssh(russh::Error),
    HostKeyMismatch { expected: String, got: String },
}

impl From<russh::Error> for SshErr {
    fn from(e: russh::Error) -> Self {
        SshErr::Ssh(e)
    }
}

struct Client {
    expected: Option<String>,
    /// Fingerprint the server presented (filled during key exchange).
    seen: Arc<Mutex<Option<String>>>,
}

impl client::Handler for Client {
    type Error = SshErr;

    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
        let fp = fingerprint(key);
        *self.seen.lock() = Some(fp.clone());
        match &self.expected {
            None => Ok(true),
            Some(exp) if fingerprints_match(exp, &fp) => Ok(true),
            Some(exp) => Err(SshErr::HostKeyMismatch { expected: exp.clone(), got: fp }),
        }
    }
}

/// OpenSSH-style fingerprint: `SHA256:<unpadded base64>`.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// Lenient comparison: tolerates a missing `SHA256:` prefix, base64 padding and
/// surrounding whitespace. The hash itself is compared exactly (base64 is case-sensitive).
fn fingerprints_match(expected: &str, got: &str) -> bool {
    fn canon(s: &str) -> String {
        let s = s.trim();
        let s = s.strip_prefix("SHA256:").or_else(|| s.strip_prefix("sha256:")).unwrap_or(s);
        s.trim_end_matches('=').to_string()
    }
    let e = canon(expected);
    !e.is_empty() && e == canon(got)
}

// ---------------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------------

fn map_ssh(e: SshErr) -> AppError {
    match e {
        SshErr::HostKeyMismatch { expected, got } => AppError::Auth(format!(
            "host key mismatch: expected {expected}, server presented {got}. \
             The server key changed — this could be a man-in-the-middle attack. \
             If the change is legitimate, clear the trusted host key and reconnect."
        )),
        SshErr::Ssh(e) => map_russh(e),
    }
}

fn map_russh(e: russh::Error) -> AppError {
    use russh::Error as E;
    match e {
        E::IO(io) => AppError::Connection(io.to_string()),
        E::UnknownKey => AppError::Auth("server host key rejected".into()),
        E::NotAuthenticated | E::NoAuthMethod => AppError::Auth(e.to_string()),
        E::Keys(k) => key_error(k),
        E::Disconnect
        | E::HUP
        | E::ConnectionTimeout
        | E::KeepaliveTimeout
        | E::InactivityTimeout
        | E::SendError
        | E::RecvError
        | E::Elapsed(_)
        | E::ChannelOpenFailure(_) => AppError::Connection(e.to_string()),
        E::Kex | E::KexInit | E::NoCommonAlgo { .. } | E::Version | E::UnknownAlgo | E::WrongServerSig => {
            AppError::Connection(format!("SSH negotiation failed: {e}"))
        }
        other => AppError::Connection(other.to_string()),
    }
}

fn key_error(e: russh::keys::Error) -> AppError {
    match e {
        russh::keys::Error::KeyIsEncrypted => {
            AppError::Auth("the private key is encrypted: a passphrase is required".into())
        }
        russh::keys::Error::IO(io) => AppError::Invalid(format!("cannot read private key: {io}")),
        other => AppError::Auth(format!("invalid private key or passphrase: {other}")),
    }
}

fn map_sftp(e: SftpError, path: &str) -> AppError {
    match e {
        SftpError::Status(st) => {
            let msg = if st.error_message.trim().is_empty() {
                format!("{path}: {}", st.status_code)
            } else {
                format!("{path}: {}", st.error_message)
            };
            match st.status_code {
                StatusCode::NoSuchFile => AppError::NotFound(msg),
                StatusCode::PermissionDenied => AppError::PermissionDenied(msg),
                StatusCode::NoConnection | StatusCode::ConnectionLost => AppError::Connection(msg),
                StatusCode::OpUnsupported => AppError::Unsupported(msg),
                _ if says_exists(&st.error_message) => AppError::AlreadyExists(msg),
                _ => AppError::Other(msg),
            }
        }
        SftpError::Timeout => AppError::Connection(format!("{path}: SFTP request timed out")),
        SftpError::IO(m) => AppError::Connection(format!("{path}: {m}")),
        SftpError::UnexpectedBehavior(m) if m.contains("session closed") || m.contains("SendError") => {
            AppError::Connection(format!("{path}: SFTP session closed"))
        }
        other => AppError::Other(format!("{path}: {other}")),
    }
}

/// SFTPv3 has no "file exists" status; servers report it as a generic failure whose
/// message usually contains "exists".
fn says_exists(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("exist") && !m.contains("not exist") && !m.contains("no such")
}

// ---------------------------------------------------------------------------------
// Connect
// ---------------------------------------------------------------------------------

fn opt<'a>(o: &'a Opts, k: &str) -> Option<&'a str> {
    o.get(k).map(|s| s.trim()).filter(|s| !s.is_empty())
}

fn expand_tilde(p: &str) -> std::path::PathBuf {
    if p == "~" {
        if let Some(h) = dirs::home_dir() {
            return h;
        }
    } else if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        if let Some(h) = dirs::home_dir() {
            return h.join(rest);
        }
    }
    std::path::PathBuf::from(p)
}

#[derive(Debug, PartialEq)]
enum AuthMethod {
    Password,
    Key,
}

fn auth_method(opts: &Opts, secrets: &Opts) -> AppResult<AuthMethod> {
    match opt(opts, "authMethod").map(|s| s.to_ascii_lowercase()) {
        Some(m) if m == "password" => Ok(AuthMethod::Password),
        Some(m) if m == "key" || m == "publickey" => Ok(AuthMethod::Key),
        Some(m) => Err(AppError::Invalid(format!("unknown authMethod: {m}"))),
        None if opt(secrets, "privateKey").is_some() || opt(opts, "keyPath").is_some() => Ok(AuthMethod::Key),
        None => Ok(AuthMethod::Password),
    }
}

fn load_key(opts: &Opts, secrets: &Opts) -> AppResult<PrivateKey> {
    let pass = opt(secrets, "passphrase");
    // Key content may be pasted with surrounding whitespace; keep inner newlines.
    if let Some(pem) = secrets.get("privateKey").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return russh::keys::decode_secret_key(pem, pass).map_err(key_error);
    }
    let path = opt(opts, "keyPath").ok_or_else(|| AppError::Invalid("a private key (keyPath or privateKey) is required".into()))?;
    let path = expand_tilde(path);
    let pem = std::fs::read_to_string(&path)
        .map_err(|e| AppError::Invalid(format!("cannot read private key {}: {e}", path.display())))?;
    russh::keys::decode_secret_key(&pem, pass).map_err(key_error)
}

async fn authenticate(handle: &mut client::Handle<Client>, user: &str, opts: &Opts, secrets: &Opts) -> AppResult<()> {
    match auth_method(opts, secrets)? {
        AuthMethod::Key => {
            let key = load_key(opts, secrets)?;
            let hash = if key.algorithm().is_rsa() {
                handle.best_supported_rsa_hash().await.map_err(map_russh)?.flatten()
            } else {
                None
            };
            let res = handle
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(map_russh)?;
            if !res.success() {
                return Err(AppError::Auth(format!("public key rejected by the server for user {user}")));
            }
        }
        AuthMethod::Password => {
            // Passwords are used verbatim (no trimming).
            let password = secrets
                .get("password")
                .map(String::as_str)
                .filter(|p| !p.is_empty())
                .ok_or_else(|| AppError::Auth("a password is required".into()))?;
            let res = handle.authenticate_password(user, password).await.map_err(map_russh)?;
            match res {
                client::AuthResult::Success => {}
                client::AuthResult::Failure { remaining_methods, .. }
                    if remaining_methods.contains(&russh::MethodKind::KeyboardInteractive) =>
                {
                    // Servers with `PasswordAuthentication no` often still accept the
                    // password through keyboard-interactive.
                    if !keyboard_interactive(handle, user, password).await? {
                        return Err(AppError::Auth(format!("wrong username or password for {user}")));
                    }
                }
                client::AuthResult::Failure { .. } => {
                    return Err(AppError::Auth(format!("wrong username or password for {user}")));
                }
            }
        }
    }
    Ok(())
}

async fn keyboard_interactive(handle: &mut client::Handle<Client>, user: &str, password: &str) -> AppResult<bool> {
    use client::KeyboardInteractiveAuthResponse as R;
    let mut resp = handle.authenticate_keyboard_interactive_start(user, None).await.map_err(map_russh)?;
    for _ in 0..5 {
        match resp {
            R::Success => return Ok(true),
            R::Failure { .. } => return Ok(false),
            R::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                resp = handle.authenticate_keyboard_interactive_respond(answers).await.map_err(map_russh)?;
            }
        }
    }
    Ok(false)
}

pub struct SftpFs {
    sftp: Arc<SftpSession>,
    root: String,
    // Kept alive for the lifetime of the provider; dropping it tears the connection down.
    _ssh: client::Handle<Client>,
}

impl SftpFs {
    /// Connects and authenticates. Returns the host key fingerprint when it was
    /// accepted for the first time (trust-on-first-use) so the caller can persist it.
    pub async fn connect(opts: &Opts, secrets: &Opts) -> AppResult<(SftpFs, Option<String>)> {
        let host = opt(opts, "host").ok_or_else(|| AppError::Invalid("missing required field: host".into()))?;
        let user = opt(opts, "username").ok_or_else(|| AppError::Invalid("missing required field: username".into()))?;
        let port: u16 = match opt(opts, "port") {
            Some(p) => p.parse().map_err(|_| AppError::Invalid(format!("invalid port: {p}")))?,
            None => 22,
        };
        // Validate local inputs before touching the network.
        if auth_method(opts, secrets)? == AuthMethod::Key {
            load_key(opts, secrets)?;
        }
        let expected = opt(opts, "hostKey").map(str::to_string);

        let config = Arc::new(client::Config {
            keepalive_interval: Some(KEEPALIVE_INTERVAL),
            keepalive_max: KEEPALIVE_MAX,
            inactivity_timeout: None,
            nodelay: true,
            // Larger receive window so parallel reads aren't throttled by flow control.
            window_size: 8 * 1024 * 1024,
            ..Default::default()
        });
        let seen = Arc::new(Mutex::new(None));
        let handler = Client { expected: expected.clone(), seen: seen.clone() };
        let host_s = host.to_string();
        let mut handle = tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config, (host_s.as_str(), port), handler))
            .await
            .map_err(|_| AppError::Connection(format!("timed out connecting to {host}:{port}")))?
            .map_err(map_ssh)?;

        tokio::time::timeout(AUTH_TIMEOUT, authenticate(&mut handle, user, opts, secrets))
            .await
            .map_err(|_| AppError::Connection("timed out during authentication".into()))??;

        let sftp = tokio::time::timeout(CONNECT_TIMEOUT, async {
            let channel = handle.channel_open_session().await.map_err(map_russh)?;
            channel.request_subsystem(true, "sftp").await.map_err(map_russh)?;
            let cfg = russh_sftp::client::Config {
                max_concurrent_writes: MAX_CONCURRENT_WRITES,
                request_timeout_secs: REQUEST_TIMEOUT_SECS,
                ..Default::default()
            };
            SftpSession::new_with_config(channel.into_stream(), cfg)
                .await
                .map_err(|e| AppError::Connection(format!("SFTP subsystem unavailable: {e}")))
        })
        .await
        .map_err(|_| AppError::Connection("timed out starting the SFTP subsystem".into()))??;

        let root = match opt(opts, "root") {
            None => sftp.canonicalize(".").await.map_err(|e| map_sftp(e, "."))?,
            Some(r) if r.starts_with('/') => r.to_string(),
            // Relative (or ~-prefixed) roots are resolved against the remote home.
            Some(r) => {
                let rel = r.trim_start_matches('~').trim_start_matches('/');
                let rel = if rel.is_empty() { "." } else { rel };
                sftp.canonicalize(rel).await.map_err(|e| map_sftp(e, r))?
            }
        };
        let root = posix::normalize(&root);

        let learned = if expected.is_none() { seen.lock().clone() } else { None };
        Ok((SftpFs { sftp: Arc::new(sftp), root, _ssh: handle }, learned))
    }

    fn entry_from(&self, path: &str, name: &str, meta: &Metadata) -> Entry {
        let is_dir = meta.is_dir();
        let mut e = Entry::new(name, path, is_dir);
        e.size = if is_dir { 0 } else { meta.size.unwrap_or(0) };
        e.modified = meta.mtime.map(|t| t as i64 * 1000);
        e.readonly = meta.permissions.is_some_and(|p| p & 0o222 == 0);
        e
    }

    /// stat that follows symlinks but still reports `is_symlink`.
    async fn stat_entry(&self, norm: &str) -> AppResult<Entry> {
        let name = if norm == "/" { "/".to_string() } else { posix::file_name(norm) };
        metrics::add_read(Channel::Net, META_WIRE_BYTES);
        let lmeta = self.sftp.symlink_metadata(norm).await.map_err(|e| map_sftp(e, norm))?;
        if lmeta.is_symlink() {
            return Ok(match self.sftp.metadata(norm).await {
                Ok(m) => {
                    let mut e = self.entry_from(norm, &name, &m);
                    e.is_symlink = true;
                    e
                }
                // Dangling link: show it as a (zero-size) file.
                Err(_) => {
                    let mut e = self.entry_from(norm, &name, &lmeta);
                    e.is_symlink = true;
                    e
                }
            });
        }
        Ok(self.entry_from(norm, &name, &lmeta))
    }

    fn remove_tree<'a>(&'a self, path: String, is_dir: bool) -> BoxFuture<'a, AppResult<()>> {
        Box::pin(async move {
            if !is_dir {
                return self.sftp.remove_file(path.as_str()).await.map_err(|e| map_sftp(e, &path));
            }
            let dir = self.sftp.read_dir(path.as_str()).await.map_err(|e| map_sftp(e, &path))?;
            let children: Vec<(String, bool)> = dir
                .filter(|d| {
                    let n = d.file_name();
                    n != "." && n != ".."
                })
                // readdir attrs are lstat-like: a symlink to a dir is removed as a link.
                .map(|d| (posix::join(&path, &d.file_name()), d.file_type().is_dir()))
                .collect();
            let results: Vec<AppResult<()>> = futures::stream::iter(children)
                .map(|(p, d)| self.remove_tree(p, d))
                .buffer_unordered(16)
                .collect()
                .await;
            for r in results {
                match r {
                    Ok(()) | Err(AppError::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            self.sftp.remove_dir(path.as_str()).await.map_err(|e| map_sftp(e, &path))
        })
    }
}

#[async_trait]
impl Provider for SftpFs {
    fn is_remote(&self) -> bool {
        true
    }

    fn caps(&self) -> Caps {
        Caps { rename: true, trash: false, thumbnails: false, server_copy: false }
    }

    fn root(&self) -> String {
        self.root.clone()
    }

    async fn list(&self, path: &str) -> AppResult<Vec<Entry>> {
        let base = posix::normalize(path);
        let dir = self.sftp.read_dir(base.as_str()).await.map_err(|e| map_sftp(e, &base))?;
        let mut out = Vec::new();
        let mut links = Vec::new();
        for d in dir {
            let name = d.file_name();
            if name == "." || name == ".." || name.is_empty() {
                continue;
            }
            let full = posix::join(&base, &name);
            let meta = d.metadata();
            if meta.is_symlink() {
                links.push(out.len());
                let mut e = self.entry_from(&full, &name, &meta);
                e.is_symlink = true;
                e.size = 0;
                out.push(e);
            } else {
                out.push(self.entry_from(&full, &name, &meta));
            }
        }
        metrics::add_read(Channel::Net, (out.len() as u64).max(1) * META_WIRE_BYTES);
        // Resolve symlink targets (dir or file?) with bounded concurrency.
        if !links.is_empty() {
            let resolved: Vec<(usize, Option<Metadata>)> = futures::stream::iter(links)
                .map(|i| {
                    let p = out[i].path.clone();
                    let sftp = self.sftp.clone();
                    async move { (i, sftp.metadata(p).await.ok()) }
                })
                .buffer_unordered(16)
                .collect()
                .await;
            for (i, m) in resolved {
                if let Some(m) = m {
                    let (name, path) = (out[i].name.clone(), out[i].path.clone());
                    let mut e = self.entry_from(&path, &name, &m);
                    e.is_symlink = true;
                    out[i] = e;
                }
            }
        }
        Ok(out)
    }

    async fn stat(&self, path: &str) -> AppResult<Entry> {
        self.stat_entry(&posix::normalize(path)).await
    }

    async fn mkdir(&self, path: &str) -> AppResult<()> {
        let norm = posix::normalize(path);
        // Fast path: already there, or only the last component is missing.
        match self.sftp.metadata(norm.as_str()).await {
            Ok(m) if m.is_dir() => return Ok(()),
            Ok(_) => return Err(AppError::AlreadyExists(norm)),
            Err(_) => {}
        }
        if self.sftp.create_dir(norm.as_str()).await.is_ok() {
            return Ok(());
        }
        // mkdir -p: walk from the top, creating what's missing.
        let mut acc = String::new();
        for seg in norm.split('/').filter(|s| !s.is_empty()) {
            acc.push('/');
            acc.push_str(seg);
            match self.sftp.metadata(acc.as_str()).await {
                Ok(m) if m.is_dir() => continue,
                Ok(_) => return Err(AppError::AlreadyExists(acc)),
                Err(SftpError::Status(st)) if st.status_code == StatusCode::NoSuchFile => {}
                Err(e) => return Err(map_sftp(e, &acc)),
            }
            if let Err(e) = self.sftp.create_dir(acc.as_str()).await {
                // Lost a race with a concurrent mkdir of the same folder? Fine.
                match self.sftp.metadata(acc.as_str()).await {
                    Ok(m) if m.is_dir() => {}
                    _ => return Err(map_sftp(e, &acc)),
                }
            }
        }
        Ok(())
    }

    async fn remove(&self, path: &str) -> AppResult<()> {
        let norm = posix::normalize(path);
        if norm == "/" {
            return Err(AppError::Invalid("refusing to remove /".into()));
        }
        // lstat: never recurse through a symlink into its target.
        let meta = self.sftp.symlink_metadata(norm.as_str()).await.map_err(|e| map_sftp(e, &norm))?;
        self.remove_tree(norm, meta.is_dir()).await
    }

    async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        let (from, to) = (posix::normalize(from), posix::normalize(to));
        if self.exists(&to).await? {
            return Err(AppError::AlreadyExists(to));
        }
        self.sftp.rename(from.as_str(), to.as_str()).await.map_err(|e| map_sftp(e, &from))
    }

    async fn reader(&self, path: &str) -> AppResult<BoxReader> {
        let norm = posix::normalize(path);
        let meta = self.sftp.metadata(norm.as_str()).await.map_err(|e| map_sftp(e, &norm))?;
        if meta.is_dir() {
            return Err(AppError::Invalid(format!("{norm} is a directory")));
        }
        let size = meta.size.unwrap_or(0) as usize;
        let streams = size.div_ceil(READ_CHUNK).clamp(1, READ_STREAMS);
        let opens = futures::future::join_all((0..streams).map(|_| self.sftp.open(norm.as_str()))).await;
        let mut files = Vec::with_capacity(streams);
        for f in opens {
            files.push(f.map_err(|e| map_sftp(e, &norm))?);
        }
        Ok(Box::new(CountingReader::new(ParallelReader::spawn(files, READ_CHUNK), Channel::Net)))
    }

    async fn writer(&self, path: &str) -> AppResult<BoxWriter> {
        let norm = posix::normalize(path);
        let file = self.sftp.create(norm.as_str()).await.map_err(|e| map_sftp(e, &norm))?;
        Ok(Box::new(CountingWriter::new(BufWriter::with_capacity(IO_CHUNK, file), Channel::Net)))
    }

    async fn usage(&self) -> AppResult<Option<Usage>> {
        match self.sftp.fs_info(self.root.as_str()).await {
            Ok(Some(s)) => {
                let unit = if s.fragment_size > 0 { s.fragment_size } else { s.block_size };
                Ok(Some(Usage { total: s.blocks * unit, free: s.blocks_avail * unit }))
            }
            _ => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------------
// Parallel, in-order reader
// ---------------------------------------------------------------------------------

/// Reads a remote file with N handles. Handle `h` fetches chunks `h, h+N, h+2N…`;
/// the consumer takes chunk `k` from stream `k % N`, so bytes come out in order. A
/// chunk shorter than `chunk` marks the end of the file.
///
/// The fetch tasks are never aborted: when the reader is dropped they notice the closed
/// channel, stop, and close their handle exactly once. (Aborting a task in the middle of
/// `File::shutdown` makes russh-sftp send a second CLOSE for the same handle from `Drop`;
/// servers such as OpenSSH reuse handle ids, so that stray CLOSE would close whatever
/// handle was opened next — e.g. a concurrent directory listing.)
struct ParallelReader {
    rxs: Vec<mpsc::Receiver<io::Result<Vec<u8>>>>,
    next: usize,
    cur: Vec<u8>,
    pos: usize,
    chunk: usize,
    done: bool,
}

impl ParallelReader {
    fn spawn(files: Vec<File>, chunk: usize) -> Self {
        let n = files.len();
        let mut rxs = Vec::with_capacity(n);
        for (h, mut file) in files.into_iter().enumerate() {
            let (tx, rx) = mpsc::channel(1);
            rxs.push(rx);
            tokio::spawn(async move {
                let mut k = h;
                loop {
                    let res = tokio::select! {
                        r = read_chunk(&mut file, (k * chunk) as u64, chunk) => r,
                        _ = tx.closed() => break,
                    };
                    let stop = !matches!(&res, Ok(b) if b.len() == chunk);
                    if tx.send(res).await.is_err() || stop {
                        break;
                    }
                    k += n;
                }
                let _ = file.shutdown().await;
            });
        }
        Self { rxs, next: 0, cur: Vec::new(), pos: 0, chunk, done: false }
    }
}

async fn read_chunk(file: &mut File, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset)).await?;
    let mut buf = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        let n = file.read(&mut buf[filled..]).await?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    buf.truncate(filled);
    Ok(buf)
}

impl AsyncRead for ParallelReader {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        loop {
            if this.pos < this.cur.len() {
                let n = (this.cur.len() - this.pos).min(out.remaining());
                out.put_slice(&this.cur[this.pos..this.pos + n]);
                this.pos += n;
                return Poll::Ready(Ok(()));
            }
            if this.done || this.rxs.is_empty() {
                return Poll::Ready(Ok(()));
            }
            let i = this.next;
            match this.rxs[i].poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(buf))) => {
                    if buf.len() < this.chunk {
                        this.done = true;
                    }
                    this.cur = buf;
                    this.pos = 0;
                    this.next = (i + 1) % this.rxs.len();
                }
                Poll::Ready(Some(Err(e))) => {
                    this.done = true;
                    return Poll::Ready(Err(e));
                }
                Poll::Ready(None) => {
                    this.done = true;
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::UnexpectedEof, "SFTP read stream ended early")));
                }
            }
        }
    }
}

#[allow(dead_code)]
fn _assert_send_sync() {
    fn is<T: Send + Sync>() {}
    is::<SftpFs>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh_sftp::protocol::Status;

    fn status(code: StatusCode, msg: &str) -> SftpError {
        SftpError::Status(Status { id: 1, status_code: code, error_message: msg.into(), language_tag: "en".into() })
    }

    // Throwaway test keys (ssh-keygen -t ed25519); K2 is encrypted with passphrase "pw".
    const K1: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACBeNmz6/ZdkscWGXa4T2XE5e2M1SOqongVC8VmokVZAQQAAAIhLA6QASwOk
AAAAAAtzc2gtZWQyNTUxOQAAACBeNmz6/ZdkscWGXa4T2XE5e2M1SOqongVC8VmokVZAQQ
AAAEA67QDJVX9Bq0AdP7hGj/kaa/zXgMbOqTBSyMWamo8lql42bPr9l2SxxYZdrhPZcTl7
YzVI6qieBULxWaiRVkBBAAAABHRlc3QB
-----END OPENSSH PRIVATE KEY-----
";
    const K1_FP: &str = "SHA256:VHzY/NcvGl50IqFvCXhaQbzK9wMlhsUDcUaTfKvChVs";
    const K2: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABDog6uAcO
TYWcQ263vl0Bo1AAAAGAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIMm80plzyc2/AsT+
9nqv6WILW1/YdX45GKGeS2NTB0NEAAAAkC2txM+Z5oKT5g4fbEPx0j5f/WyL5TCfTdAJ6y
PhCWjsM8vP2xrLQHwW7E6h/3dzmxccqseVQrc7k0xd9ivbYWSDFQyGyCearnqyoCwa0BsE
37IuxPiyXumor1kalQ7aZxC3tsliLTp2lPE8/TmFqbC2XVGus29wx6MG5yolHO4NTGIRfy
UbB2mjL3Fw4xfGCg==
-----END OPENSSH PRIVATE KEY-----
";
    const K2_FP: &str = "SHA256:9+pa1fUDMFc0gvkS3Cf0lgJtowGMDmUTPkPG8gLCFSA";

    #[test]
    fn fingerprint_format_and_matching() {
        let key = russh::keys::decode_secret_key(K1, None).unwrap();
        let fp = fingerprint(key.public_key());
        // Same format as `ssh-keygen -lf`.
        assert_eq!(fp, K1_FP);
        assert!(fingerprints_match(&fp, &fp));
        assert!(fingerprints_match(&format!("  {fp} "), &fp));
        assert!(fingerprints_match(fp.trim_start_matches("SHA256:"), &fp));
        assert!(fingerprints_match(&format!("{fp}="), &fp));
        assert!(!fingerprints_match(K2_FP, &fp));
        assert!(!fingerprints_match("", &fp));
        assert!(!fingerprints_match("SHA256:", &fp));
        // Base64 is case-sensitive: a case-altered fingerprint is a different key.
        assert!(!fingerprints_match(&fp.to_ascii_lowercase(), &fp));
    }

    #[test]
    fn sftp_status_mapping() {
        assert!(matches!(map_sftp(status(StatusCode::NoSuchFile, "No such file"), "/x"), AppError::NotFound(_)));
        assert!(matches!(map_sftp(status(StatusCode::PermissionDenied, ""), "/x"), AppError::PermissionDenied(_)));
        assert!(matches!(map_sftp(status(StatusCode::Failure, "File exists"), "/x"), AppError::AlreadyExists(_)));
        assert!(matches!(map_sftp(status(StatusCode::Failure, "Failure"), "/x"), AppError::Other(_)));
        assert!(matches!(map_sftp(status(StatusCode::Failure, "file does not exist"), "/x"), AppError::Other(_)));
        assert!(matches!(map_sftp(status(StatusCode::ConnectionLost, ""), "/x"), AppError::Connection(_)));
        assert!(matches!(map_sftp(status(StatusCode::OpUnsupported, ""), "/x"), AppError::Unsupported(_)));
        assert!(matches!(map_sftp(SftpError::Timeout, "/x"), AppError::Connection(_)));
        let msg = map_sftp(status(StatusCode::NoSuchFile, "No such file"), "/a/b").to_string();
        assert!(msg.contains("/a/b"), "{msg}");
    }

    #[test]
    fn ssh_error_mapping() {
        let e = map_ssh(SshErr::HostKeyMismatch { expected: "SHA256:aaa".into(), got: "SHA256:bbb".into() });
        assert!(matches!(&e, AppError::Auth(m) if m.contains("host key mismatch")));
        assert!(matches!(map_russh(russh::Error::Disconnect), AppError::Connection(_)));
        assert!(matches!(map_russh(russh::Error::UnknownKey), AppError::Auth(_)));
        assert!(matches!(
            map_russh(russh::Error::IO(io::Error::from(io::ErrorKind::ConnectionRefused))),
            AppError::Connection(_)
        ));
        assert!(matches!(key_error(russh::keys::Error::KeyIsEncrypted), AppError::Auth(_)));
    }

    #[test]
    fn auth_method_resolution() {
        let mut o = Opts::new();
        let mut s = Opts::new();
        assert_eq!(auth_method(&o, &s).unwrap(), AuthMethod::Password);
        s.insert("privateKey".into(), "-----BEGIN…".into());
        assert_eq!(auth_method(&o, &s).unwrap(), AuthMethod::Key);
        o.insert("authMethod".into(), "password".into());
        assert_eq!(auth_method(&o, &s).unwrap(), AuthMethod::Password);
        o.insert("authMethod".into(), "bogus".into());
        assert!(auth_method(&o, &s).is_err());
        let mut o2 = Opts::new();
        o2.insert("keyPath".into(), "~/.ssh/id_ed25519".into());
        assert_eq!(auth_method(&o2, &Opts::new()).unwrap(), AuthMethod::Key);
    }

    #[test]
    fn key_loading() {
        let mut s = Opts::new();
        s.insert("privateKey".into(), format!("\n{K1}\n"));
        let loaded = load_key(&Opts::new(), &s).unwrap();
        assert_eq!(fingerprint(loaded.public_key()), K1_FP);

        let mut s = Opts::new();
        s.insert("privateKey".into(), K2.into());
        assert!(matches!(load_key(&Opts::new(), &s), Err(AppError::Auth(m)) if m.contains("passphrase")));
        s.insert("passphrase".into(), "wrong".into());
        assert!(matches!(load_key(&Opts::new(), &s), Err(AppError::Auth(_))));
        s.insert("passphrase".into(), "pw".into());
        assert_eq!(fingerprint(load_key(&Opts::new(), &s).unwrap().public_key()), K2_FP);

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("id");
        std::fs::write(&p, K1).unwrap();
        let mut o = Opts::new();
        o.insert("keyPath".into(), p.to_string_lossy().into_owned());
        assert!(load_key(&o, &Opts::new()).is_ok());
        o.insert("keyPath".into(), dir.path().join("missing").to_string_lossy().into_owned());
        assert!(matches!(load_key(&o, &Opts::new()), Err(AppError::Invalid(_))));
    }

    #[test]
    fn tilde_expansion() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_tilde("~/.ssh/id"), home.join(".ssh/id"));
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("/abs/id"), std::path::PathBuf::from("/abs/id"));
    }

    #[tokio::test]
    async fn parallel_reader_reassembles_in_order() {
        // Exercise the consumer side with fake producer channels.
        let chunk = 4;
        let data: Vec<u8> = (0u8..19).collect();
        let n = 3;
        let mut rxs = Vec::new();
        let mut txs = Vec::new();
        for _ in 0..n {
            let (tx, rx) = mpsc::channel(8);
            rxs.push(rx);
            txs.push(tx);
        }
        for (k, c) in data.chunks(chunk).enumerate() {
            txs[k % n].send(Ok(c.to_vec())).await.unwrap();
        }
        let mut r = ParallelReader { rxs, next: 0, cur: vec![], pos: 0, chunk, done: false };
        let mut out = Vec::new();
        r.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, data);
    }
}
