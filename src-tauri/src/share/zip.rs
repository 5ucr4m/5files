//! Streaming ZIP writer for "download folder": files are stored (not compressed) and
//! streamed straight from the provider, so a folder of any size starts downloading at
//! once and costs no extra disk. Sizes are known up front (from the listing), which
//! makes the exact archive length computable — the browser gets a Content-Length and
//! shows a real progress bar. CRCs go in data descriptors after each file; ZIP64
//! records are used only where a size or offset needs them.

use crate::error::{AppError, AppResult};
use crate::vfs::DynProvider;
use bytes::Bytes;
use chrono::{Datelike, Local, TimeZone, Timelike};
use tokio::io::AsyncReadExt;

const LOCAL_SIG: u32 = 0x0403_4b50;
const DESCRIPTOR_SIG: u32 = 0x0807_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD64_SIG: u32 = 0x0606_4b50;
const LOCATOR64_SIG: u32 = 0x0706_4b50;
const MAX32: u64 = 0xFFFF_FFFF;
/// Bit 3: sizes/CRC follow the data. Bit 11: names are UTF-8.
const FLAG_STREAMED: u16 = 0x0808;
const FLAG_UTF8: u16 = 0x0800;

#[derive(Clone, Debug)]
pub struct Item {
    /// Archive path with `/` separators; folders end with `/`.
    pub name: String,
    /// Provider path of a file; `None` for a folder entry.
    pub source: Option<String>,
    pub size: u64,
    /// Unix epoch milliseconds.
    pub modified: Option<i64>,
}

impl Item {
    fn is_dir(&self) -> bool {
        self.source.is_none()
    }
    fn zip64(&self) -> bool {
        self.size >= MAX32
    }
}

/// Positions of every entry plus the exact archive length.
pub struct Plan {
    items: Vec<Item>,
    offsets: Vec<u64>,
    central_start: u64,
    central_len: u64,
    pub total: u64,
}

/// ZIP64 entries carry a (zeroed) ZIP64 extra in the local header too: readers that
/// stream use its presence to expect 8-byte sizes in the descriptor (APPNOTE 4.3.9.2).
fn local_len(it: &Item) -> u64 {
    30 + it.name.len() as u64 + if it.zip64() { 20 } else { 0 }
}

fn descriptor_len(it: &Item) -> u64 {
    match (it.is_dir(), it.zip64()) {
        (true, _) => 0,
        (false, false) => 16,
        (false, true) => 24,
    }
}

fn central_extra_len(it: &Item, offset: u64) -> u64 {
    let n = if it.zip64() { 16 } else { 0 } + if offset >= MAX32 { 8 } else { 0 };
    if n == 0 { 0 } else { 4 + n }
}

pub fn plan(items: Vec<Item>) -> Plan {
    let mut offsets = Vec::with_capacity(items.len());
    let mut pos = 0u64;
    for it in &items {
        offsets.push(pos);
        pos += local_len(it) + if it.is_dir() { 0 } else { it.size } + descriptor_len(it);
    }
    let central_start = pos;
    let central_len: u64 = items.iter().zip(&offsets).map(|(it, &off)| 46 + it.name.len() as u64 + central_extra_len(it, off)).sum();
    let end_len = if needs_zip64_end(items.len(), central_start, central_len) { 56 + 20 + 22 } else { 22 };
    Plan { total: central_start + central_len + end_len, items, offsets, central_start, central_len }
}

fn needs_zip64_end(count: usize, start: u64, len: u64) -> bool {
    count >= 0xFFFF || start >= MAX32 || len >= MAX32
}

fn dos_time(ms: Option<i64>) -> (u16, u16) {
    let Some(dt) = ms.and_then(|ms| Local.timestamp_millis_opt(ms).single()) else {
        return (0, (1 << 5) | 1); // 1980-01-01
    };
    if dt.year() < 1980 {
        return (0, (1 << 5) | 1);
    }
    let time = ((dt.hour() << 11) | (dt.minute() << 5) | (dt.second() / 2)) as u16;
    let date = ((((dt.year() - 1980).min(127) as u32) << 9) | (dt.month() << 5) | dt.day()) as u16;
    (time, date)
}

struct Buf(Vec<u8>);
impl Buf {
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
}

fn local_header(it: &Item) -> Vec<u8> {
    let (time, date) = dos_time(it.modified);
    let mut b = Buf(Vec::with_capacity(local_len(it) as usize));
    b.u32(LOCAL_SIG);
    b.u16(if it.zip64() { 45 } else { 20 });
    b.u16(if it.is_dir() { FLAG_UTF8 } else { FLAG_STREAMED });
    b.u16(0); // stored
    b.u16(time);
    b.u16(date);
    b.u32(0); // crc, sizes: in the descriptor
    let size32 = if it.zip64() { MAX32 as u32 } else { 0 };
    b.u32(size32);
    b.u32(size32);
    b.u16(it.name.len() as u16);
    b.u16(if it.zip64() { 20 } else { 0 });
    b.0.extend_from_slice(it.name.as_bytes());
    if it.zip64() {
        b.u16(0x0001);
        b.u16(16);
        b.u64(0);
        b.u64(0);
    }
    b.0
}

fn descriptor(it: &Item, crc: u32) -> Vec<u8> {
    let mut b = Buf(Vec::with_capacity(24));
    b.u32(DESCRIPTOR_SIG);
    b.u32(crc);
    if it.zip64() {
        b.u64(it.size);
        b.u64(it.size);
    } else {
        b.u32(it.size as u32);
        b.u32(it.size as u32);
    }
    b.0
}

fn central(it: &Item, crc: u32, offset: u64) -> Vec<u8> {
    let (time, date) = dos_time(it.modified);
    let big_size = it.zip64();
    let big_off = offset >= MAX32;
    let mut b = Buf(Vec::with_capacity(64 + it.name.len()));
    b.u32(CENTRAL_SIG);
    b.u16(0x0300 | 45); // made by: Unix, spec 4.5
    b.u16(if big_size || big_off { 45 } else { 20 });
    b.u16(if it.is_dir() { FLAG_UTF8 } else { FLAG_STREAMED });
    b.u16(0);
    b.u16(time);
    b.u16(date);
    b.u32(crc);
    let size32 = if big_size { MAX32 as u32 } else { it.size as u32 };
    b.u32(size32);
    b.u32(size32);
    b.u16(it.name.len() as u16);
    b.u16(central_extra_len(it, offset) as u16);
    b.u16(0); // comment
    b.u16(0); // disk
    b.u16(0); // internal attributes
    let mode: u32 = if it.is_dir() { 0o040755 } else { 0o100644 };
    b.u32(mode << 16 | if it.is_dir() { 0x10 } else { 0 });
    b.u32(if big_off { MAX32 as u32 } else { offset as u32 });
    b.0.extend_from_slice(it.name.as_bytes());
    if big_size || big_off {
        b.u16(0x0001);
        b.u16((if big_size { 16 } else { 0 } + if big_off { 8 } else { 0 }) as u16);
        if big_size {
            b.u64(it.size);
            b.u64(it.size);
        }
        if big_off {
            b.u64(offset);
        }
    }
    b.0
}

fn end_records(count: usize, start: u64, len: u64) -> Vec<u8> {
    let mut b = Buf(Vec::with_capacity(98));
    let zip64 = needs_zip64_end(count, start, len);
    if zip64 {
        let at = start + len;
        b.u32(EOCD64_SIG);
        b.u64(44);
        b.u16(0x0300 | 45);
        b.u16(45);
        b.u32(0);
        b.u32(0);
        b.u64(count as u64);
        b.u64(count as u64);
        b.u64(len);
        b.u64(start);
        b.u32(LOCATOR64_SIG);
        b.u32(0);
        b.u64(at);
        b.u32(1);
    }
    b.u32(EOCD_SIG);
    b.u16(0);
    b.u16(0);
    let n16 = if zip64 { 0xFFFF } else { count as u16 };
    b.u16(n16);
    b.u16(n16);
    b.u32(if zip64 { MAX32 as u32 } else { len as u32 });
    b.u32(if zip64 { MAX32 as u32 } else { start as u32 });
    b.u16(0);
    b.0
}

/// Streams the archive into `out`. Fails (so the download is aborted, never silently
/// corrupt) if a file got shorter than its listed size meanwhile; extra bytes of a file
/// that grew are left out.
pub async fn write(plan: Plan, provider: DynProvider, out: tokio::sync::mpsc::Sender<Bytes>) -> AppResult<()> {
    const CHUNK: usize = 256 * 1024;
    let send = |b: Vec<u8>| {
        let out = out.clone();
        async move { out.send(Bytes::from(b)).await.map_err(|_| AppError::Cancelled) }
    };
    let mut crcs = Vec::with_capacity(plan.items.len());
    for it in &plan.items {
        send(local_header(it)).await?;
        let Some(src) = &it.source else {
            crcs.push(0);
            continue;
        };
        let mut r = provider.reader(src).await?.take(it.size);
        let mut h = crc32fast::Hasher::new();
        let mut left = it.size;
        while left > 0 {
            let mut buf = vec![0u8; CHUNK.min(left as usize)];
            let n = r.read(&mut buf).await?;
            if n == 0 {
                return Err(AppError::Other(format!("{} changed while it was being downloaded", it.name)));
            }
            buf.truncate(n);
            h.update(&buf);
            left -= n as u64;
            send(buf).await?;
        }
        let crc = h.finalize();
        crcs.push(crc);
        send(descriptor(it, crc)).await?;
    }
    let mut cd = Vec::with_capacity(plan.central_len as usize);
    for ((it, crc), off) in plan.items.iter().zip(&crcs).zip(&plan.offsets) {
        cd.extend_from_slice(&central(it, *crc, *off));
    }
    debug_assert_eq!(cd.len() as u64, plan.central_len);
    send(cd).await?;
    send(end_records(plan.items.len(), plan.central_start, plan.central_len)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn build(dir: &std::path::Path, items: Vec<Item>) -> Vec<u8> {
        let plan = plan(items);
        let total = plan.total;
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let p: DynProvider = Arc::new(crate::vfs::local::LocalFs);
        let task = tokio::spawn(write(plan, p, tx));
        let mut out = Vec::new();
        while let Some(b) = rx.recv().await {
            out.extend_from_slice(&b);
        }
        task.await.unwrap().unwrap();
        assert_eq!(out.len() as u64, total, "planned length matches the bytes written");
        let _ = dir;
        out
    }

    #[tokio::test]
    async fn archive_is_valid_and_exactly_sized() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("fotos ünï.bin");
        std::fs::write(&a, b"hello zip").unwrap();
        std::fs::write(&b, vec![7u8; 600_000]).unwrap();
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        let items = vec![
            Item { name: "share/".into(), source: None, size: 0, modified: Some(1_700_000_000_000) },
            Item { name: "share/a.txt".into(), source: Some(s(&a)), size: 9, modified: Some(1_700_000_000_000) },
            Item { name: "share/sub/".into(), source: None, size: 0, modified: None },
            Item { name: "share/sub/fotos ünï.bin".into(), source: Some(s(&b)), size: 600_000, modified: None },
        ];
        let bytes = build(dir.path(), items).await;
        let zip = dir.path().join("out.zip");
        std::fs::write(&zip, &bytes).unwrap();
        // Cross-check with the system unzip where available (macOS, Linux CI).
        if let Ok(o) = std::process::Command::new("unzip").arg("-t").arg(&zip).output() {
            let log = String::from_utf8_lossy(&o.stdout);
            assert!(o.status.success(), "unzip -t failed:\n{log}");
            assert!(log.contains("No errors detected"), "{log}");
        }
        // The end record points at the central directory, which lists all four entries.
        let eocd = &bytes[bytes.len() - 22..];
        assert_eq!(u32::from_le_bytes(eocd[0..4].try_into().unwrap()), EOCD_SIG);
        assert_eq!(u16::from_le_bytes(eocd[10..12].try_into().unwrap()), 4);
    }

    #[tokio::test]
    async fn file_that_shrank_aborts_the_stream() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        std::fs::write(&a, b"short").unwrap();
        let items = vec![Item { name: "a.txt".into(), source: Some(a.to_string_lossy().into()), size: 100, modified: None }];
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let p: DynProvider = Arc::new(crate::vfs::local::LocalFs);
        let task = tokio::spawn(write(plan(items), p, tx));
        while rx.recv().await.is_some() {}
        assert!(task.await.unwrap().is_err());
    }

    #[test]
    fn zip64_sizes_switch_record_formats() {
        let big = Item { name: "big.iso".into(), source: Some("/x".into()), size: 5 << 30, modified: None };
        let p = plan(vec![big.clone(), Item { name: "after.txt".into(), source: Some("/y".into()), size: 1, modified: None }]);
        // The second entry starts beyond 4 GiB, so its central record carries an offset extra.
        assert!(p.offsets[1] >= MAX32);
        assert_eq!(central(&p.items[1], 0, p.offsets[1]).len() as u64, 46 + 9 + 12);
        assert_eq!(descriptor(&big, 0).len(), 24);
        assert_eq!(p.total, p.central_start + p.central_len + 98);
    }
}
