// Live data pushed by the backend: transfer queue, I/O metrics, drives & connections.
import { create } from "zustand";
import { api } from "../lib/api";
import type { Connection, Job, MetricsSnapshot, Place, Rates, Volume } from "../lib/types";

export const HISTORY = 120; // 60 s at 500 ms

const zero: Rates = { diskRead: 0, diskWrite: 0, netRx: 0, netTx: 0 };

interface MetricsState {
  history: Rates[];
  latest: MetricsSnapshot | null;
  push: (m: MetricsSnapshot) => void;
}

export const useMetrics = create<MetricsState>((set) => ({
  history: Array.from({ length: HISTORY }, () => zero),
  latest: null,
  push: (m) => set((s) => ({ latest: m, history: [...s.history.slice(1 - HISTORY), m.rates] })),
}));

interface TransfersState {
  jobs: Job[];
  set: (jobs: Job[]) => void;
}

export const useTransfers = create<TransfersState>((set) => ({
  jobs: [],
  set: (jobs) => set({ jobs }),
}));

export const isActive = (j: Job) => !["done", "failed", "cancelled"].includes(j.status);

/** Aggregate progress across active jobs (0..1), or null when idle. */
export function aggregate(jobs: Job[]) {
  const active = jobs.filter(isActive);
  if (!active.length) return null;
  const total = active.reduce((t, j) => t + j.totalBytes, 0);
  const done = active.reduce((t, j) => t + j.doneBytes, 0);
  const speed = active.reduce((t, j) => t + j.speed, 0);
  return { count: active.length, progress: total > 0 ? done / total : 0, speed, eta: speed > 0 ? (total - done) / speed : null };
}

interface DrivesState {
  volumes: Volume[];
  places: Place[];
  connections: Connection[];
  refreshVolumes: () => Promise<void>;
  refreshConnections: () => Promise<void>;
  setStatus: (id: string, status: Connection["status"], error?: string | null) => void;
}

export const useDrives = create<DrivesState>((set) => ({
  volumes: [],
  places: [],
  connections: [],
  async refreshVolumes() {
    const [volumes, places] = await Promise.all([api().volumes(), api().places()]);
    set({ volumes, places });
  },
  async refreshConnections() {
    set({ connections: await api().connections() });
  },
  setStatus: (id, status, error = null) =>
    set((s) => ({ connections: s.connections.map((c) => (c.id === id ? { ...c, status, error } : c)) })),
}));
