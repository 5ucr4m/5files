//! Folder sharing on the local network: a folder (on any drive) is served by a small
//! built-in web server so another computer or phone can browse it in a browser,
//! download files or whole folders as ZIP, and — when allowed — add files.
//!
//! Each share gets an unguessable token that is part of its link
//! (`http://192.168.1.20:5155/s/<token>/`); without the token nothing is reachable.
//! One server (one port) serves every active share; it starts with the first share
//! and stops with the last. Shares live until stopped, expired or 5files quits.

pub mod http;
pub mod zip;

use crate::vfs::DynProvider;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;

pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// First choice of port (stable links across restarts of the server); the next few are
/// tried before falling back to any free port.
const PREFERRED_PORT: u16 = 5155;
const ACTIVITY_KEEP: usize = 60;
/// A browser re-opening the page within this window is not logged again.
const REOPEN_QUIET_MS: i64 = 30 * 60 * 1000;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShareOptions {
    /// Visitors may add files (never delete or overwrite).
    pub allow_upload: bool,
    pub show_hidden: bool,
    /// Stop sharing automatically after this many minutes (from now).
    pub expires_in_mins: Option<u64>,
    /// Visitors must type this (4–8 digits) before they see anything.
    #[serde(default)]
    pub pin: Option<String>,
}

pub fn valid_pin(pin: &str) -> bool {
    (4..=8).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_digit())
}

/// Wrong PINs allowed from one address before it has to wait.
const PIN_TRIES: u32 = 5;

/// Outcome of a PIN attempt.
pub enum PinCheck {
    /// Right PIN: the visitor's new session id (goes in a cookie).
    Ok(String),
    Wrong {
        tries_left: u32,
    },
    /// Too many wrong tries: seconds until this address may try again.
    Locked(u64),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub at: i64,
    pub who: String,
    /// "opened" | "unlocked" | "wrongPin" | "downloaded" | "zip" | "uploaded"
    pub action: &'static str,
    pub name: String,
}

#[derive(Default)]
struct Stats {
    visitors: HashMap<IpAddr, i64>,
    activity: VecDeque<Activity>,
    last_seen: Option<i64>,
}

pub struct Share {
    pub token: String,
    pub name: String,
    pub fs: String,
    pub root: String,
    pub provider: DynProvider,
    pub created: i64,
    opts: Mutex<(ShareOptions, Option<i64>)>,
    stats: Mutex<Stats>,
    /// Visitors who entered the PIN (session ids); cleared when the PIN changes.
    sessions: Mutex<std::collections::HashSet<String>>,
    /// Wrong PIN attempts per address: (count, locked until ms).
    attempts: Mutex<HashMap<IpAddr, (u32, i64)>>,
    pub bytes_out: AtomicU64,
    pub bytes_in: AtomicU64,
    /// Downloads / uploads in flight right now.
    pub active: AtomicU32,
    /// Cancelled when the share stops: in-flight transfers end with it.
    pub stopped: CancellationToken,
    notify: Notify,
}

impl Share {
    pub fn options(&self) -> ShareOptions {
        self.opts.lock().0.clone()
    }

    pub fn needs_pin(&self) -> bool {
        self.opts.lock().0.pin.is_some()
    }

    /// Whether a visitor's session (from their cookie) may see the share.
    pub fn admitted(&self, session: Option<&str>) -> bool {
        !self.needs_pin() || session.is_some_and(|s| self.sessions.lock().contains(s))
    }

    /// Checks a PIN typed by a visitor. Five wrong tries lock that address out for 30 s,
    /// doubling with every further round (up to 15 min), so guessing isn't practical.
    pub fn try_pin(&self, ip: IpAddr, typed: &str) -> PinCheck {
        let Some(pin) = self.opts.lock().0.pin.clone() else {
            return PinCheck::Ok(new_session());
        };
        let now = now_ms();
        let mut attempts = self.attempts.lock();
        let entry = attempts.entry(ip).or_insert((0, 0));
        if entry.1 > now {
            return PinCheck::Locked(((entry.1 - now) as u64).div_ceil(1000));
        }
        // Compare without an early exit (no timing hints).
        let same = typed.len() == pin.len() && typed.bytes().zip(pin.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
        if same {
            attempts.remove(&ip);
            drop(attempts);
            let session = new_session();
            self.sessions.lock().insert(session.clone());
            self.log(ip, "unlocked", "");
            return PinCheck::Ok(session);
        }
        entry.0 += 1;
        let tries_left = PIN_TRIES - (entry.0 - 1) % PIN_TRIES - 1;
        let lock_secs = (tries_left == 0).then(|| {
            let round = entry.0 / PIN_TRIES;
            (30u64 << (round - 1).min(5)).min(15 * 60)
        });
        if let Some(secs) = lock_secs {
            entry.1 = now + secs as i64 * 1000;
        }
        drop(attempts);
        self.log(ip, "wrongPin", "");
        match lock_secs {
            Some(secs) => PinCheck::Locked(secs),
            None => PinCheck::Wrong { tries_left },
        }
    }

    fn expires_at(&self) -> Option<i64> {
        self.opts.lock().1
    }

    pub fn touch(&self, ip: IpAddr) {
        let mut s = self.stats.lock();
        let t = now_ms();
        s.visitors.insert(ip, t);
        s.last_seen = Some(t);
    }

    pub fn log(&self, ip: IpAddr, action: &'static str, name: impl Into<String>) {
        let mut s = self.stats.lock();
        let at = now_ms();
        // A browser re-opening the page soon after isn't a new visit.
        if action == "opened"
            && let Some(prev) = s.visitors.get(&ip)
            && at - prev < REOPEN_QUIET_MS
            && s.activity.iter().any(|a| a.action == "opened" && a.who == ip.to_string())
        {
            s.visitors.insert(ip, at);
            return;
        }
        s.visitors.insert(ip, at);
        s.last_seen = Some(at);
        if s.activity.len() == ACTIVITY_KEEP {
            s.activity.pop_back();
        }
        s.activity.push_front(Activity { at, who: ip.to_string(), action, name: name.into() });
        drop(s);
        (self.notify)();
    }

    pub fn add_out(&self, n: u64) {
        self.bytes_out.fetch_add(n, Ordering::Relaxed);
        (self.notify)();
    }

    pub fn add_in(&self, n: u64) {
        self.bytes_in.fetch_add(n, Ordering::Relaxed);
        (self.notify)();
    }

    /// Counts a transfer as active until the guard drops.
    pub fn busy(self: &Arc<Self>) -> BusyGuard {
        self.active.fetch_add(1, Ordering::Relaxed);
        (self.notify)();
        BusyGuard(self.clone())
    }
}

pub struct BusyGuard(Arc<Share>);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
        (self.0.notify)();
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Qr {
    pub size: usize,
    /// Row-major modules, `1` = dark.
    pub bits: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareView {
    pub id: String,
    pub name: String,
    pub fs: String,
    pub path: String,
    pub url: String,
    /// Other ways to reach the same share (e.g. `name.local`).
    pub alt_urls: Vec<String>,
    /// False when this computer isn't on a network: only it can open the link.
    pub online: bool,
    pub qr: Qr,
    pub options: ShareOptions,
    pub expires_at: Option<i64>,
    pub created: i64,
    pub visitors: usize,
    pub last_seen: Option<i64>,
    pub bytes_out: u64,
    pub bytes_in: u64,
    pub active: u32,
    pub activity: Vec<Activity>,
}

struct Server {
    port: u16,
    cancel: CancellationToken,
}

pub struct Shares {
    shares: Mutex<HashMap<String, Arc<Share>>>,
    server: tokio::sync::Mutex<Option<Server>>,
    port: AtomicU32,
    notify: Notify,
    pub thumbs_dir: PathBuf,
}

/// The address other devices on the network reach this computer at: the source address
/// of the default route (a UDP "connect" sends nothing).
pub fn lan_ip() -> Option<IpAddr> {
    for target in ["8.8.8.8:80", "192.168.0.1:80", "10.0.0.1:80"] {
        if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0")
            && s.connect(target).is_ok()
            && let Ok(a) = s.local_addr()
            && !a.ip().is_loopback()
            && !a.ip().is_unspecified()
        {
            return Some(a.ip());
        }
    }
    None
}

fn command_line(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// "Ana's MacBook Pro" — what the page says the folder is shared from.
pub fn computer_name() -> &'static str {
    static NAME: once_cell::sync::Lazy<String> = once_cell::sync::Lazy::new(|| {
        #[cfg(target_os = "macos")]
        if let Some(n) = command_line("scutil", &["--get", "ComputerName"]) {
            return n;
        }
        #[cfg(windows)]
        if let Ok(n) = std::env::var("COMPUTERNAME") {
            return n;
        }
        if let Ok(n) = std::fs::read_to_string("/etc/hostname")
            && !n.trim().is_empty()
        {
            return n.trim().to_string();
        }
        command_line("hostname", &[]).unwrap_or_else(|| "this computer".into())
    });
    &NAME
}

/// Bonjour name (`ana-mbp.local`) where the OS advertises one: survives IP changes.
fn mdns_host() -> Option<&'static str> {
    static HOST: once_cell::sync::Lazy<Option<String>> = once_cell::sync::Lazy::new(|| {
        #[cfg(target_os = "macos")]
        {
            command_line("scutil", &["--get", "LocalHostName"]).map(|h| format!("{h}.local"))
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    });
    HOST.as_deref()
}

fn check_options(o: &ShareOptions) -> crate::error::AppResult<()> {
    match &o.pin {
        Some(p) if !valid_pin(p) => Err(crate::error::AppError::Invalid("The PIN must be 4 to 8 digits.".into())),
        _ => Ok(()),
    }
}

fn new_session() -> String {
    use rand::RngCore;
    let mut b = [0u8; 24];
    rand::rng().fill_bytes(&mut b);
    hex::encode(b)
}

fn new_token() -> String {
    use rand::Rng;
    // No look-alikes (0/o, 1/l): links get read aloud and typed on phones. 12 × 5 bits.
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    let mut rng = rand::rng();
    (0..12).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char).collect()
}

pub fn qr(text: &str) -> Qr {
    match qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M) {
        Ok(code) => {
            Qr { size: code.width(), bits: code.to_colors().iter().map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' }).collect() }
        }
        Err(_) => Qr { size: 0, bits: String::new() },
    }
}

impl Shares {
    pub fn new(thumbs_dir: PathBuf, notify: Notify) -> Arc<Self> {
        Arc::new(Self { shares: Default::default(), server: Default::default(), port: AtomicU32::new(0), notify, thumbs_dir })
    }

    pub fn get(&self, token: &str) -> Option<Arc<Share>> {
        self.shares.lock().get(token).cloned()
    }

    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed) as u16
    }

    pub fn count(&self) -> usize {
        self.shares.lock().len()
    }

    async fn ensure_server(self: &Arc<Self>) -> std::io::Result<u16> {
        let mut srv = self.server.lock().await;
        if let Some(s) = srv.as_ref() {
            return Ok(s.port);
        }
        let forced = std::env::var("FIVEFILES_SHARE_PORT").ok().and_then(|p| p.parse::<u16>().ok());
        let mut listener = None;
        let candidates: Vec<u16> = match forced {
            Some(p) => vec![p],
            None => (PREFERRED_PORT..PREFERRED_PORT + 10).chain([0]).collect(),
        };
        let mut last_err = None;
        for port in candidates {
            match std::net::TcpListener::bind(("0.0.0.0", port)) {
                Ok(l) => {
                    listener = Some(l);
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        let listener = listener.ok_or_else(|| last_err.unwrap_or_else(|| std::io::Error::other("no free port")))?;
        let port = listener.local_addr()?.port();
        // Registered with the reactor inside the server task (see `http::serve`).
        listener.set_nonblocking(true)?;
        let cancel = CancellationToken::new();
        tauri::async_runtime::spawn(http::serve(listener, self.clone(), cancel.clone()));
        self.port.store(port as u32, Ordering::Relaxed);
        *srv = Some(Server { port, cancel });
        Ok(port)
    }

    async fn stop_server_if_idle(&self) {
        let mut srv = self.server.lock().await;
        if self.count() == 0
            && let Some(s) = srv.take()
        {
            s.cancel.cancel();
            self.port.store(0, Ordering::Relaxed);
        }
    }

    pub async fn start(
        self: &Arc<Self>,
        fs: String,
        root: String,
        name: String,
        provider: DynProvider,
        opts: ShareOptions,
    ) -> crate::error::AppResult<ShareView> {
        // Sharing the same folder twice just returns the existing link, as it is.
        let existing = self.shares.lock().values().find(|s| s.fs == fs && s.root == root).cloned();
        if let Some(s) = existing {
            return Ok(self.view_of(&s, &Addresses::now()));
        }
        check_options(&opts)?;
        self.ensure_server().await.map_err(|e| crate::error::AppError::Other(format!("couldn't start the sharing server: {e}")))?;
        let token = new_token();
        let share = Arc::new(Share {
            token: token.clone(),
            name,
            fs,
            root,
            provider,
            created: now_ms(),
            opts: Mutex::new((opts.clone(), opts.expires_in_mins.map(|m| now_ms() + m as i64 * 60_000))),
            stats: Default::default(),
            sessions: Default::default(),
            attempts: Default::default(),
            bytes_out: AtomicU64::new(0),
            bytes_in: AtomicU64::new(0),
            active: AtomicU32::new(0),
            stopped: CancellationToken::new(),
            notify: self.notify.clone(),
        });
        self.shares.lock().insert(token.clone(), share.clone());
        (self.notify)();
        Ok(self.view_of(&share, &Addresses::now()))
    }

    pub fn update(&self, token: &str, opts: ShareOptions) -> crate::error::AppResult<ShareView> {
        let share = self.get(token).ok_or_else(|| crate::error::AppError::NotFound("share".into()))?;
        check_options(&opts)?;
        {
            let mut o = share.opts.lock();
            // A new PIN signs everyone out.
            if o.0.pin != opts.pin {
                share.sessions.lock().clear();
                share.attempts.lock().clear();
            }
            // Only a changed expiry restarts the countdown.
            let expires =
                if o.0.expires_in_mins == opts.expires_in_mins { o.1 } else { opts.expires_in_mins.map(|m| now_ms() + m as i64 * 60_000) };
            *o = (opts, expires);
        }
        (self.notify)();
        Ok(self.view_of(&share, &Addresses::now()))
    }

    pub async fn stop(&self, token: &str) {
        let removed = self.shares.lock().remove(token);
        if let Some(s) = removed {
            s.stopped.cancel();
            (self.notify)();
        }
        self.stop_server_if_idle().await;
    }

    pub async fn stop_all(&self) {
        let all: Vec<_> = self.shares.lock().drain().map(|(_, s)| s).collect();
        for s in &all {
            s.stopped.cancel();
        }
        (self.notify)();
        self.stop_server_if_idle().await;
    }

    /// Stops shares whose time ran out. Returns whether any did.
    pub async fn expire(&self) -> bool {
        let t = now_ms();
        let due: Vec<String> =
            self.shares.lock().values().filter(|s| s.expires_at().is_some_and(|e| e <= t)).map(|s| s.token.clone()).collect();
        for token in &due {
            self.stop(token).await;
        }
        !due.is_empty()
    }

    /// Names and in-flight transfer counts (cheap; for the tray).
    pub fn brief(&self) -> Vec<(String, u32)> {
        let mut v: Vec<_> = self.shares.lock().values().map(|s| (s.created, s.name.clone(), s.active.load(Ordering::Relaxed))).collect();
        v.sort();
        v.into_iter().map(|(_, n, a)| (n, a)).collect()
    }

    pub fn views(&self) -> Vec<ShareView> {
        let addr = Addresses::now();
        let mut out: Vec<_> = self.shares.lock().values().map(|s| self.view_of(s, &addr)).collect();
        out.sort_by_key(|v| v.created);
        out
    }

    fn view_of(&self, s: &Share, addr: &Addresses) -> ShareView {
        let port = self.port();
        let link = |host: &str| format!("http://{host}:{port}/s/{}/", s.token);
        let url = link(&addr.primary);
        let (opts, expires_at) = s.opts.lock().clone();
        let st = s.stats.lock();
        ShareView {
            id: s.token.clone(),
            name: s.name.clone(),
            fs: s.fs.clone(),
            path: s.root.clone(),
            qr: qr(&url),
            url,
            alt_urls: addr.alt.iter().map(|h| link(h)).collect(),
            online: addr.online,
            options: opts,
            expires_at,
            created: s.created,
            visitors: st.visitors.len(),
            last_seen: st.last_seen,
            bytes_out: s.bytes_out.load(Ordering::Relaxed),
            bytes_in: s.bytes_in.load(Ordering::Relaxed),
            active: s.active.load(Ordering::Relaxed),
            activity: st.activity.iter().take(20).cloned().collect(),
        }
    }
}

struct Addresses {
    primary: String,
    alt: Vec<String>,
    online: bool,
}

impl Addresses {
    fn now() -> Self {
        let ip = lan_ip();
        let alt = mdns_host().filter(|_| ip.is_some()).map(|h| vec![h.to_string()]).unwrap_or_default();
        Addresses { primary: ip.map(|i| i.to_string()).unwrap_or_else(|| "127.0.0.1".into()), alt, online: ip.is_some() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_and_unambiguous() {
        let t = new_token();
        assert_eq!(t.len(), 12);
        assert!(t.chars().all(|c| !"01lo".contains(c)));
        assert_ne!(t, new_token());
    }

    #[test]
    fn qr_encodes_a_link() {
        let q = qr("http://192.168.100.200:5155/s/abcdefghijkm/");
        assert!(q.size >= 25);
        assert_eq!(q.bits.len(), q.size * q.size);
    }
}
