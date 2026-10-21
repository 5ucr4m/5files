import { AnimatePresence, LayoutGroup, animate, motion, useMotionValue, useReducedMotion, useTransform } from "motion/react";
import { useEffect, useRef } from "react";
import { clampRatio, type LayoutNode } from "../lib/layout";
import { isMac } from "../lib/platform";
import { useLayout, type Workspace as Ws } from "../store/layout";
import { useSettings } from "../store/settings";
import { useTabs } from "../store/tabs";
import { FileView } from "./FileView";
import { StatusBar } from "./StatusBar";
import { Toolbar } from "./Toolbar";

const SPRING = { type: "spring", stiffness: 420, damping: 40, mass: 0.9 } as const;

function PaneFrame({ ws, paneId, multi }: { ws: Ws; paneId: string; multi: boolean }) {
  const tab = useTabs((s) => s.tabs.find((t) => t.id === paneId));
  const focused = ws.focus === paneId;
  if (!tab) return null;
  return (
    <section
      data-pane-id={paneId}
      aria-label="Pane"
      onMouseDownCapture={() => !focused && useLayout.getState().focusPane(paneId)}
      className={`@container relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-xl border bg-surface shadow-soft transition-[border-color] duration-200 ${
        multi && focused ? "border-[color-mix(in_oklab,var(--accent)_38%,var(--line))]" : "border-line"
      }`}
    >
      {multi && focused && (
        // The focus beam: one element shared across panes, so it glides to the newly focused pane.
        <motion.div
          layoutId={`focus-beam-${ws.id}`}
          transition={SPRING}
          className="pointer-events-none absolute inset-x-3 top-0 z-30 h-[2px] rounded-b-full bg-accent shadow-[0_0_14px_var(--accent)]"
        />
      )}
      <div className={`flex min-h-0 flex-1 flex-col transition-opacity duration-200 ${multi && !focused ? "[&_header]:opacity-70" : ""}`}>
        <Toolbar tab={tab} focused={focused} />
        <div className="relative min-h-0 flex-1">
          {tab.loading && !tab.listing && <div className="indeterminate absolute inset-x-0 top-0 z-20 h-[2px] overflow-hidden" />}
          <FileView key={tab.id} tab={tab} focused={focused} />
        </div>
        <StatusBar tab={tab} />
      </div>
    </section>
  );
}

function Gutter({ ws, node, onDrag }: { ws: Ws; node: Extract<LayoutNode, { kind: "split" }>; onDrag: (r: number | null) => void }) {
  const row = node.dir === "row";
  return (
    <div
      role="separator"
      aria-orientation={row ? "vertical" : "horizontal"}
      title="Drag to resize · double-click to even out"
      onMouseDown={(e) => {
        e.preventDefault();
        const box = (e.currentTarget.parentElement as HTMLElement).getBoundingClientRect();
        const at = (ev: MouseEvent) => clampRatio(row ? (ev.clientX - box.left) / box.width : (ev.clientY - box.top) / box.height);
        const move = (ev: MouseEvent) => onDrag(at(ev));
        const up = (ev: MouseEvent) => {
          window.removeEventListener("mousemove", move);
          window.removeEventListener("mouseup", up);
          document.body.style.cursor = "";
          useLayout.getState().setRatio(ws.id, node.id, at(ev));
          onDrag(null);
        };
        document.body.style.cursor = row ? "col-resize" : "row-resize";
        window.addEventListener("mousemove", move);
        window.addEventListener("mouseup", up);
      }}
      onDoubleClick={() => useLayout.getState().setRatio(ws.id, node.id, 0.5)}
      className={`group relative z-20 flex shrink-0 items-center justify-center ${row ? "w-[6px] cursor-col-resize" : "h-[6px] cursor-row-resize"}`}
    >
      {/* Hairline + grab pill appear on hover only: gutters stay quiet until you reach for them. */}
      <span className={`absolute rounded-full bg-accent opacity-0 transition-opacity duration-150 group-hover:opacity-60 ${row ? "inset-y-2 w-px" : "inset-x-2 h-px"}`} />
      <span className={`rounded-full bg-accent opacity-0 transition-opacity duration-150 group-hover:opacity-100 ${row ? "h-8 w-[4px]" : "h-[4px] w-8"}`} />
    </div>
  );
}

function SplitView({ ws, node, multi }: { ws: Ws; node: Extract<LayoutNode, { kind: "split" }>; multi: boolean }) {
  const reduce = useReducedMotion() || useSettings.getState().reduceMotion;
  // A new split starts fully given to the existing pane and springs open.
  const ratio = useMotionValue(node.born ? 1 : node.ratio);
  const dragging = useRef(false);
  const target = node.closing === "a" ? 0 : node.closing === "b" ? 1 : node.ratio;
  const row = node.dir === "row";

  useEffect(() => {
    if (dragging.current) return;
    const ctrl = animate(ratio, target, reduce ? { duration: 0 } : SPRING);
    let live = true;
    void ctrl.then(() => {
      if (!live) return;
      const st = useLayout.getState();
      if (node.closing) {
        const gone = node[node.closing];
        if (gone.kind === "pane") st.finishClose(ws.id, gone.id);
      } else if (node.born) st.settle(ws.id, node.id);
    });
    return () => {
      live = false;
      ctrl.stop();
    };
  }, [target, node.closing, node.born]); // eslint-disable-line react-hooks/exhaustive-deps

  const aGrow = useTransform(ratio, (r) => r);
  const bGrow = useTransform(ratio, (r) => 1 - r);
  // Content fades as its side collapses, and the incoming pane slides in from its edge.
  const aOpacity = useTransform(ratio, [0, 0.12], [0, 1]);
  const bOpacity = useTransform(ratio, [0.88, 1], [1, 0]);
  const bShift = useTransform(ratio, [0.5, 1], [0, 28]);

  return (
    <div className={`flex min-h-0 min-w-0 flex-1 ${row ? "flex-row" : "flex-col"}`}>
      <motion.div style={{ flexGrow: aGrow, flexBasis: 0, opacity: aOpacity }} className="flex min-h-0 min-w-0 overflow-hidden">
        <NodeView ws={ws} node={node.a} multi={multi} />
      </motion.div>
      {node.closing ? (
        <div className={row ? "w-[6px] shrink-0" : "h-[6px] shrink-0"} />
      ) : (
        <Gutter
          ws={ws}
          node={node}
          onDrag={(r) => {
            dragging.current = r !== null;
            if (r !== null) ratio.set(r);
          }}
        />
      )}
      <motion.div
        style={{ flexGrow: bGrow, flexBasis: 0, opacity: bOpacity, x: row ? bShift : 0, y: row ? 0 : bShift }}
        className="flex min-h-0 min-w-0 overflow-hidden"
      >
        <NodeView ws={ws} node={node.b} multi={multi} />
      </motion.div>
    </div>
  );
}

function NodeView({ ws, node, multi }: { ws: Ws; node: LayoutNode; multi: boolean }) {
  // Keyed by node id: a split's spring state must never carry over to a different split.
  return node.kind === "pane" ? (
    <PaneFrame key={node.id} ws={ws} paneId={node.id} multi={multi} />
  ) : (
    <SplitView key={node.id} ws={ws} node={node} multi={multi} />
  );
}

export function Workspace() {
  const ws = useLayout((s) => s.workspaces.find((w) => w.id === s.active));
  if (!ws) return null;
  const multi = ws.root.kind === "split";
  return (
    <LayoutGroup id={ws.id}>
      <div className="flex min-h-0 min-w-0 flex-1">
        <AnimatePresence mode="popLayout" initial={false}>
          {ws.zoom ? (
            <motion.div
              key={`zoom-${ws.zoom}`}
              initial={{ opacity: 0, scale: 0.97 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.98 }}
              transition={SPRING}
              className="flex min-h-0 min-w-0 flex-1"
            >
              <PaneFrame ws={ws} paneId={ws.zoom} multi={false} />
            </motion.div>
          ) : (
            <motion.div key={`tree-${ws.id}`} initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="flex min-h-0 min-w-0 flex-1">
              <NodeView ws={ws} node={ws.root} multi={multi} />
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </LayoutGroup>
  );
}

const PREFIX_KEYS: [string, string][] = [
  ["%", "split right"],
  ['"', "split down"],
  ["← ↑ → ↓", "move"],
  ["o", "next pane"],
  ["z", "zoom"],
  ["x", "close pane"],
  ["c", "new tab"],
];

/** Shown while the tmux prefix (Ctrl+B) waits for its command key. */
export function PrefixHud() {
  const on = useLayout((s) => s.prefix);
  return (
    <AnimatePresence>
      {on && (
        <motion.div
          initial={{ opacity: 0, y: 14, scale: 0.97 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: 8, transition: { duration: 0.12 } }}
          transition={SPRING}
          className="pointer-events-none fixed bottom-10 left-1/2 z-[97] flex -translate-x-1/2 items-center gap-3 rounded-2xl border border-line bg-elevated px-3.5 py-2.5 shadow-pop"
          role="status"
          aria-live="polite"
        >
          <kbd className="num flex h-7 items-center rounded-lg border border-accent bg-sel px-2 text-[12px] font-semibold text-accent">{isMac() ? "⌃B" : "Ctrl+B"}</kbd>
          <span className="h-5 w-px bg-line" />
          {PREFIX_KEYS.map(([k, label]) => (
            <span key={k} className="flex items-center gap-1.5 text-[11.5px] whitespace-nowrap text-ink-3">
              <kbd className="num rounded-md bg-surface-3 px-1.5 py-0.5 text-[11px] font-medium text-ink">{k}</kbd>
              {label}
            </span>
          ))}
        </motion.div>
      )}
    </AnimatePresence>
  );
}
