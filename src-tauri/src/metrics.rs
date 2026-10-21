//! App-scoped I/O accounting.
//!
//! Every byte Atlas reads from / writes to a local disk or sends / receives over the
//! network flows through the counting wrappers below, so the numbers reflect *only*
//! this app's activity (not system-wide traffic), and the measurement is identical on
//! macOS, Windows and Linux.

use serde::Serialize;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Default)]
pub struct Counters {
    pub disk_read: AtomicU64,
    pub disk_write: AtomicU64,
    pub net_rx: AtomicU64,
    pub net_tx: AtomicU64,
}

pub static COUNTERS: Counters = Counters {
    disk_read: AtomicU64::new(0),
    disk_write: AtomicU64::new(0),
    net_rx: AtomicU64::new(0),
    net_tx: AtomicU64::new(0),
};

/// Which pair of counters an I/O stream should be charged to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Disk,
    Net,
}

impl Channel {
    pub fn for_remote(remote: bool) -> Self {
        if remote { Channel::Net } else { Channel::Disk }
    }
}

#[inline]
pub fn add_read(ch: Channel, n: u64) {
    match ch {
        Channel::Disk => COUNTERS.disk_read.fetch_add(n, Ordering::Relaxed),
        Channel::Net => COUNTERS.net_rx.fetch_add(n, Ordering::Relaxed),
    };
}

#[inline]
pub fn add_write(ch: Channel, n: u64) {
    match ch {
        Channel::Disk => COUNTERS.disk_write.fetch_add(n, Ordering::Relaxed),
        Channel::Net => COUNTERS.net_tx.fetch_add(n, Ordering::Relaxed),
    };
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub disk_read: u64,
    pub disk_write: u64,
    pub net_rx: u64,
    pub net_tx: u64,
}

pub fn totals() -> Totals {
    Totals {
        disk_read: COUNTERS.disk_read.load(Ordering::Relaxed),
        disk_write: COUNTERS.disk_write.load(Ordering::Relaxed),
        net_rx: COUNTERS.net_rx.load(Ordering::Relaxed),
        net_tx: COUNTERS.net_tx.load(Ordering::Relaxed),
    }
}

/// One sample of throughput in bytes/second.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rates {
    pub disk_read: f64,
    pub disk_write: f64,
    pub net_rx: f64,
    pub net_tx: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub rates: Rates,
    pub totals: Totals,
    pub peak: Rates,
}

/// Turns monotonically increasing totals into per-second rates and keeps a short
/// history so the peak over the visible window can be reported.
pub struct RateSampler {
    last: Totals,
    last_at: Instant,
    history: VecDeque<Rates>,
    cap: usize,
}

impl RateSampler {
    pub fn new(cap: usize) -> Self {
        Self { last: totals(), last_at: Instant::now(), history: VecDeque::with_capacity(cap), cap }
    }

    pub fn sample_with(&mut self, now_totals: Totals, now: Instant) -> MetricsSnapshot {
        let dt = now.duration_since(self.last_at).max(Duration::from_millis(1)).as_secs_f64();
        let d = |a: u64, b: u64| a.saturating_sub(b) as f64 / dt;
        let rates = Rates {
            disk_read: d(now_totals.disk_read, self.last.disk_read),
            disk_write: d(now_totals.disk_write, self.last.disk_write),
            net_rx: d(now_totals.net_rx, self.last.net_rx),
            net_tx: d(now_totals.net_tx, self.last.net_tx),
        };
        self.last = now_totals;
        self.last_at = now;
        if self.history.len() == self.cap {
            self.history.pop_front();
        }
        self.history.push_back(rates);
        let peak = self.history.iter().fold(Rates::default(), |p, r| Rates {
            disk_read: p.disk_read.max(r.disk_read),
            disk_write: p.disk_write.max(r.disk_write),
            net_rx: p.net_rx.max(r.net_rx),
            net_tx: p.net_tx.max(r.net_tx),
        });
        MetricsSnapshot { rates, totals: now_totals, peak }
    }

    pub fn sample(&mut self) -> MetricsSnapshot {
        self.sample_with(totals(), Instant::now())
    }
}

/// AsyncRead wrapper that charges every byte read to a channel.
pub struct CountingReader<R> {
    inner: R,
    ch: Channel,
}

impl<R> CountingReader<R> {
    pub fn new(inner: R, ch: Channel) -> Self {
        Self { inner, ch }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for CountingReader<R> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &res {
            add_read(self.ch, (buf.filled().len() - before) as u64);
        }
        res
    }
}

/// AsyncWrite wrapper that charges every byte written to a channel.
pub struct CountingWriter<W> {
    inner: W,
    ch: Channel,
}

impl<W> CountingWriter<W> {
    pub fn new(inner: W, ch: Channel) -> Self {
        Self { inner, ch }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for CountingWriter<W> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &res {
            add_write(self.ch, *n as u64);
        }
        res
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn sampler_computes_rates_from_deltas() {
        let mut s = RateSampler::new(4);
        let t0 = s.last_at;
        s.last = Totals::default();
        let snap = s.sample_with(
            Totals { disk_read: 1000, disk_write: 500, net_rx: 0, net_tx: 2000 },
            t0 + Duration::from_millis(500),
        );
        assert!((snap.rates.disk_read - 2000.0).abs() < 1.0);
        assert!((snap.rates.disk_write - 1000.0).abs() < 1.0);
        assert!((snap.rates.net_tx - 4000.0).abs() < 1.0);
        assert_eq!(snap.rates.net_rx, 0.0);
        // Next sample with no new traffic: rate drops to 0, peak is retained.
        let snap2 = s.sample_with(snap.totals, t0 + Duration::from_millis(1000));
        assert_eq!(snap2.rates.disk_read, 0.0);
        assert!((snap2.peak.disk_read - 2000.0).abs() < 1.0);
    }

    #[tokio::test]
    async fn counting_wrappers_charge_the_right_channel() {
        let before = totals();
        let data = vec![7u8; 4096];
        let mut r = CountingReader::new(&data[..], Channel::Net);
        let mut out = Vec::new();
        r.read_to_end(&mut out).await.unwrap();
        let mut w = CountingWriter::new(Vec::new(), Channel::Disk);
        w.write_all(&out).await.unwrap();
        let after = totals();
        assert!(after.net_rx - before.net_rx >= 4096);
        assert!(after.disk_write - before.disk_write >= 4096);
    }
}
