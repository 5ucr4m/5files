import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBackend, type Backend } from "./api";
import { measureFolders } from "./sizes";
import type { SizeEvent } from "./types";

describe("measureFolders", () => {
  let calls: { paths: string[]; emit: (e: SizeEvent) => void }[];
  let cancelled: string[];
  beforeEach(() => {
    vi.useFakeTimers();
    calls = [];
    cancelled = [];
    setBackend({
      folderSizes: async (_fs: string, paths: string[], _force: boolean, emit: (e: SizeEvent) => void) => {
        calls.push({ paths, emit });
        return `task${calls.length}`;
      },
      cancelSearch: async (id: string) => void cancelled.push(id),
    } as unknown as Backend);
  });
  afterEach(() => vi.useRealTimers());

  it("a second pane joins the running count instead of starting another", async () => {
    const stopA = measureFolders("local", ["/big"]);
    const stopB = measureFolders("local", ["/big"]);
    await vi.runAllTimersAsync();
    expect(calls).toHaveLength(1);
    stopA();
    await vi.advanceTimersByTimeAsync(2000);
    expect(cancelled).toEqual([]); // pane B still wants it
    stopB();
    await vi.advanceTimersByTimeAsync(2000);
    expect(cancelled).toEqual(["task1"]);
  });

  it("a listing refresh takes over the count instead of restarting it", async () => {
    const stop = measureFolders("local", ["/big"]);
    await vi.runAllTimersAsync();
    stop(); // effect cleanup on refresh…
    const again = measureFolders("local", ["/big"]); // …immediately followed by the new effect
    await vi.advanceTimersByTimeAsync(2000);
    expect(calls).toHaveLength(1);
    expect(cancelled).toEqual([]);
    again();
  });

  it("finished folders are measured again on the next request", async () => {
    measureFolders("local", ["/x"]);
    await vi.runAllTimersAsync();
    calls[0].emit({ type: "done", path: "/x", bytes: 1, files: 1 });
    measureFolders("local", ["/x"]);
    await vi.runAllTimersAsync();
    expect(calls).toHaveLength(2);
  });
});
