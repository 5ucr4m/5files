// Built-in end-to-end smoke run, enabled with `FIVEFILES_E2E=<empty dir>`. Drives the real
// UI stores (same code paths as clicks/shortcuts) inside the real webview and reports
// PASS/FAIL to the app log, covering native events, the watcher and metrics.
import { invoke } from "@tauri-apps/api/core";
import { api } from "./api";
import { newFolder, sendToOtherPane, transfer } from "./ops";
import { useLayout } from "../store/layout";
import { useMetrics, useTransfers } from "../store/live";
import { folderSize, useSizes } from "../store/sizes";
import { measureFolders } from "./sizes";
import { activeTab, useTabs } from "../store/tabs";

const log = (message: string) => invoke("frontend_log", { level: "e2e", message });
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function until<T>(what: string, fn: () => T | undefined | null | false, ms = 15000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = fn();
    if (v) return v as T;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(50);
  }
}

export async function runE2E(dir: string, home?: string) {
  const step = async (name: string, f: () => Promise<void>) => {
    await f();
    await log(`ok  ${name}`);
  };
  try {
    const sep = dir.includes("\\") ? "\\" : "/";
    const tabs = useTabs.getState();
    const tab = () => activeTab()!;
    await step("open test folder", async () => {
      await tabs.navigate(tab().id, { fs: "local", path: dir });
      await until("listing", () => tab().listing?.path === dir);
    });
    await step("create folder (inline rename starts)", async () => {
      await newFolder();
      await until("new folder in listing", () => tab().listing?.entries.some((e) => e.name === "untitled folder" && e.isDir));
      await until("rename mode", () => tab().renaming?.endsWith("untitled folder"));
      tabs.patch(tab().id, { renaming: null });
    });
    await step("watcher picks up a file created outside 5files", async () => {
      await api().createFile("local", dir, "external.txt");
      await until("watcher refresh", () => tab().listing?.entries.some((e) => e.name === "external.txt"), 8000);
    });
    await step("copy through the queue with progress events", async () => {
      // seed.bin is prepared by the harness, big enough for several progress events.
      const before = useMetrics.getState().latest?.totals.diskWrite ?? 0;
      const ok = await transfer("copy", [{ fs: "local", path: `${dir}${sep}seed.bin` }], { fs: "local", path: `${dir}${sep}untitled folder` });
      if (!ok) throw new Error("transfer was not enqueued");
      const job = await until("job done", () => useTransfers.getState().jobs.find((j) => j.status === "done" || j.status === "failed"), 30000);
      if (job.status !== "done") throw new Error(`job ${job.status}: ${job.error}`);
      if (job.doneBytes !== job.totalBytes || job.totalBytes <= 0) throw new Error(`bytes ${job.doneBytes}/${job.totalBytes}`);
      await until("metrics reflect disk writes", () => (useMetrics.getState().latest?.totals.diskWrite ?? 0) - before >= job.totalBytes, 5000);
    });
    await step("thumbnail protocol serves an image", async () => {
      // Load it the way the grid does (an <img>), which is what the CSP allows.
      const size = await new Promise<number>((resolve, reject) => {
        const img = new Image();
        img.onload = () => resolve(img.naturalWidth);
        img.onerror = () => reject(new Error("thumbnail failed to load"));
        img.src = api().thumbUrl(`${dir}${sep}pic.png`, 64);
      });
      if (size !== 64) throw new Error(`thumbnail is ${size}px, expected 64`);
    });
    await step("dropped file streams in through binary IPC chunks", async () => {
      const bytes = new Uint8Array(5 * 1024 * 1024 + 7).map((_, i) => i % 251);
      const path = await api().uploadFile("local", dir, new File([bytes], "dropped.bin"));
      await tabs.refresh(tab().id);
      const e = tab().listing?.entries.find((x) => x.path === path);
      if (!e || e.size !== bytes.length) throw new Error(`uploaded size ${e?.size}`);
    });
    await step("folder sizes are measured and streamed", async () => {
      const folder = `${dir}${sep}untitled folder`;
      measureFolders("local", [folder], true);
      const size = await until("folder size", () => {
        const s = folderSize(useSizes.getState().sizes, "local", folder);
        return s?.done ? s : null;
      });
      if (size.bytes !== 96 * 1024 * 1024 || size.files !== 1) throw new Error(`measured ${size.bytes} bytes / ${size.files} files`);
    });
    await step("split panes: open, copy to the other pane, close", async () => {
      const L = useLayout.getState();
      L.split("row");
      await until("two panes", () => document.querySelectorAll("[data-pane-id]").length === 2);
      const right = activeTab()!;
      await tabs.navigate(right.id, { fs: "local", path: `${dir}${sep}untitled folder` });
      L.next(1); // back to the first pane (the test folder)
      const left = activeTab()!;
      await until("left listing", () => left.listing?.path === dir);
      useTabs.getState().select(left.id, [`${dir}${sep}pic.png`]);
      await sendToOtherPane(useTabs.getState().tabs.find((t) => t.id === left.id)!, "copy");
      await until("copied into the other pane", () => useTabs.getState().tabs.find((t) => t.id === right.id)?.listing?.entries.some((e) => e.name === "pic.png"), 10000);
      useTabs.getState().closeTab(right.id);
      await until("back to one pane", () => document.querySelectorAll("[data-pane-id]").length === 1 && !useTabs.getState().tabs.some((t) => t.id === right.id), 5000);
    });
    if (home) {
      await step(`measures a real folder (${home})`, async () => {
        const t0 = performance.now();
        measureFolders("local", [home], true);
        const size = await until("home size", () => {
          const s = folderSize(useSizes.getState().sizes, "local", home);
          return s?.done ? s : null;
        }, 120000);
        await log(`    ${size.files.toLocaleString()} files, ${(size.bytes / 1e9).toFixed(2)} GB in ${Math.round(performance.now() - t0)} ms`);
      });
    }
    await log("PASS");
  } catch (e) {
    await log(`FAIL ${e instanceof Error ? e.message : String(e)}`);
  }
}
