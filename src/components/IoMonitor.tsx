import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { formatBytes, formatRate } from "../lib/format";
import type { Rates } from "../lib/types";
import { HISTORY, useMetrics } from "../store/live";

type Lane = "disk" | "net";
const KEYS: Record<Lane, [keyof Rates, keyof Rates]> = {
  disk: ["diskRead", "diskWrite"],
  net: ["netRx", "netTx"],
};
const SAMPLE_MS = 500;

function cssVar(el: Element, name: string) {
  return getComputedStyle(el).getPropertyValue(name).trim();
}

/**
 * Mirrored "seismograph": the upper trace is reads/downloads, the lower trace is
 * writes/uploads. It scrolls continuously between samples and auto-scales to the
 * recent peak, easing the scale so spikes don't make the trace jump.
 */
export function Scope({ lane, width, height, grid = false }: { lane: Lane; width: number; height: number; grid?: boolean }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const scale = useRef(1);
  const lastPush = useRef(performance.now());
  const history = useMetrics((s) => s.history);

  useEffect(() => {
    lastPush.current = performance.now();
  }, [history]);

  useEffect(() => {
    const canvas = ref.current!;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = width * dpr;
    canvas.height = height * dpr;
    const ctx = canvas.getContext("2d")!;
    let raf = 0;
    const [ka, kb] = KEYS[lane];

    const draw = () => {
      const h = useMetrics.getState().history;
      const colA = cssVar(canvas, "--trace-a");
      const colB = cssVar(canvas, "--trace-b");
      const line = cssVar(canvas, "--line-strong");
      const peak = Math.max(64 * 1024, ...h.slice(-60).map((r) => Math.max(r[ka], r[kb])));
      scale.current += (peak - scale.current) * 0.08;
      const mid = height / 2;
      const amp = mid - 1.5;
      const frac = Math.min(1, (performance.now() - lastPush.current) / SAMPLE_MS);
      const step = width / (HISTORY - 2);

      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, width, height);
      if (grid) {
        ctx.strokeStyle = line;
        ctx.globalAlpha = 0.5;
        ctx.setLineDash([2, 4]);
        for (const y of [mid - amp / 2, mid + amp / 2]) {
          ctx.beginPath();
          ctx.moveTo(0, y);
          ctx.lineTo(width, y);
          ctx.stroke();
        }
        ctx.setLineDash([]);
        ctx.globalAlpha = 1;
      }
      ctx.strokeStyle = line;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(0, mid + 0.5);
      ctx.lineTo(width, mid + 0.5);
      ctx.stroke();

      const trace = (key: keyof Rates, dir: 1 | -1, color: string) => {
        if (!h.some((r) => r[key] > 0)) return; // idle: just the baseline
        const pts: [number, number][] = h.map((r, i) => {
          const v = Math.sqrt(Math.min(1, r[key] / scale.current)); // sqrt: small activity stays visible
          return [(i - frac) * step, mid - dir * v * amp];
        });
        const grad = ctx.createLinearGradient(0, mid, 0, mid - dir * amp);
        grad.addColorStop(0, color + "00");
        grad.addColorStop(1, color + "55");
        ctx.beginPath();
        ctx.moveTo(pts[0][0], mid);
        for (const [x, y] of pts) ctx.lineTo(x, y);
        ctx.lineTo(pts[pts.length - 1][0], mid);
        ctx.closePath();
        ctx.fillStyle = grad;
        ctx.fill();
        ctx.beginPath();
        pts.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
        ctx.strokeStyle = color;
        ctx.lineWidth = 1.25;
        ctx.lineJoin = "round";
        ctx.stroke();
      };
      trace(ka, 1, toHex(colA, canvas));
      trace(kb, -1, toHex(colB, canvas));

      const active = h.slice(-HISTORY + 2).some((r) => r[ka] > 0 || r[kb] > 0);
      if (active) raf = requestAnimationFrame(draw);
      else raf = 0;
    };
    draw();
    const unsub = useMetrics.subscribe(() => {
      if (!raf) raf = requestAnimationFrame(draw);
    });
    const mq = matchMedia("(prefers-color-scheme: dark)");
    const redraw = () => !raf && (raf = requestAnimationFrame(draw));
    mq.addEventListener("change", redraw);
    const mo = new MutationObserver(redraw);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "style"] });
    return () => {
      cancelAnimationFrame(raf);
      unsub();
      mq.removeEventListener("change", redraw);
      mo.disconnect();
    };
  }, [lane, width, height, grid]);

  return <canvas ref={ref} style={{ width, height }} className="block" aria-hidden />;
}

/** Resolve any CSS color (incl. color-mix/var) to #rrggbb so we can append alpha. */
const hexCache = new Map<string, string>();
function toHex(color: string, el: HTMLElement): string {
  const hit = hexCache.get(color);
  if (hit) return hit;
  const probe = document.createElement("span");
  probe.style.color = color;
  el.parentElement?.appendChild(probe);
  const rgb = getComputedStyle(probe).color.match(/\d+(\.\d+)?/g)?.map(Number) ?? [120, 120, 120];
  probe.remove();
  const hex = "#" + rgb.slice(0, 3).map((n) => Math.round(n).toString(16).padStart(2, "0")).join("");
  hexCache.set(color, hex);
  if (hexCache.size > 32) hexCache.clear();
  return hex;
}

function Rate({ value, tone }: { value: number; tone: "a" | "b" }) {
  return (
    <span className="num flex items-center gap-1 text-[10.5px] leading-none text-ink-2">
      <span className="text-[8px]" style={{ color: tone === "a" ? "var(--trace-a)" : "var(--trace-b)" }}>
        {tone === "a" ? "▲" : "▼"}
      </span>
      <span className="w-[62px] truncate">{formatRate(value)}</span>
    </span>
  );
}

function Compact({ lane, label }: { lane: Lane; label: string }) {
  const latest = useMetrics((s) => s.latest);
  const [ka, kb] = KEYS[lane];
  return (
    <div className="flex items-center gap-2">
      <span className="w-6 text-[9px] font-semibold tracking-[0.12em] text-ink-3">{label}</span>
      <Scope lane={lane} width={74} height={24} />
      <div className="flex flex-col gap-[3px]">
        <Rate value={latest?.rates[ka] ?? 0} tone="a" />
        <Rate value={latest?.rates[kb] ?? 0} tone="b" />
      </div>
    </div>
  );
}

export function IoMonitor() {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !box.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  return (
    <div ref={box} className="no-drag relative">
      <button
        onClick={() => setOpen((o) => !o)}
        title="Disk and network activity from Atlas"
        className={`flex h-[34px] items-center gap-3 rounded-[9px] px-2.5 transition-colors hover:bg-chrome-active ${open ? "bg-chrome-active" : ""}`}
      >
        <Compact lane="disk" label="DSK" />
        <span className="h-5 w-px bg-line" />
        <Compact lane="net" label="NET" />
      </button>
      <AnimatePresence>{open && <MonitorPanel />}</AnimatePresence>
    </div>
  );
}

function MonitorPanel() {
  const latest = useMetrics((s) => s.latest);
  const rows: { lane: Lane; title: string; a: string; b: string }[] = [
    { lane: "disk", title: "Disk", a: "Read", b: "Write" },
    { lane: "net", title: "Network", a: "Download", b: "Upload" },
  ];
  return (
    <motion.div
      initial={{ opacity: 0, y: -6, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, y: -4, scale: 0.98 }}
      transition={{ type: "spring", stiffness: 520, damping: 34 }}
      className="absolute top-[42px] right-0 z-50 w-[340px] origin-top-right rounded-2xl border border-line bg-elevated p-4 shadow-pop"
    >
      <div className="mb-3 flex items-baseline justify-between">
        <h3 className="text-[13px] font-semibold tracking-tight">Activity</h3>
        <span className="text-2xs text-ink-3">Atlas only · last 60 s</span>
      </div>
      <div className="space-y-4">
        {rows.map(({ lane, title, a, b }) => {
          const [ka, kb] = KEYS[lane];
          return (
            <div key={lane}>
              <div className="mb-1.5 flex items-center justify-between text-2xs">
                <span className="font-medium text-ink-2">{title}</span>
                <span className="num text-ink-3">peak {formatRate(Math.max(latest?.peak[ka] ?? 0, latest?.peak[kb] ?? 0))}</span>
              </div>
              <div className="rounded-lg border border-line bg-surface-2 px-1 py-1">
                <Scope lane={lane} width={298} height={64} grid />
              </div>
              <div className="mt-2 grid grid-cols-2 gap-2">
                {([
                  [a, ka, "a"],
                  [b, kb, "b"],
                ] as const).map(([label, key, tone]) => (
                  <div key={label} className="rounded-lg bg-surface-2 px-2.5 py-2">
                    <div className="flex items-center gap-1.5 text-2xs text-ink-3">
                      <span className="size-1.5 rounded-full" style={{ background: tone === "a" ? "var(--trace-a)" : "var(--trace-b)" }} />
                      {label}
                    </div>
                    <div className="num mt-0.5 text-[13px] font-medium">{formatRate(latest?.rates[key] ?? 0)}</div>
                    <div className="num text-2xs text-ink-3">{formatBytes(latest?.totals[key] ?? 0)} total</div>
                  </div>
                ))}
              </div>
            </div>
          );
        })}
      </div>
    </motion.div>
  );
}
