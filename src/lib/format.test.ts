import { describe, expect, it } from "vitest";
import { categoryOf, formatBytes, formatDuration, freshName, kindLabel, sortEntries, stemRange } from "./format";
import type { Entry } from "./types";

const e = (name: string, over: Partial<Entry> = {}): Entry => {
  const dot = name.lastIndexOf(".");
  return {
    name,
    path: `/${name}`,
    isDir: false,
    isSymlink: false,
    size: 0,
    modified: 0,
    created: null,
    hidden: false,
    readonly: false,
    ext: dot > 0 ? name.slice(dot + 1).toLowerCase() : null,
    ...over,
  };
};

describe("formatBytes", () => {
  it("uses SI units", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(999)).toBe("999 B");
    expect(formatBytes(1500)).toBe("1.5 KB");
    expect(formatBytes(6_100_000_000)).toBe("6.1 GB");
    expect(formatBytes(512_000_000)).toBe("512 MB");
    expect(formatBytes(-1)).toBe("—");
  });
});

describe("formatDuration", () => {
  it("is compact", () => {
    expect(formatDuration(0.2)).toBe("<1s");
    expect(formatDuration(42)).toBe("42s");
    expect(formatDuration(125)).toBe("2m 5s");
    expect(formatDuration(3 * 3600 + 120)).toBe("3h 2m");
    expect(formatDuration(null)).toBe("");
  });
});

describe("categories", () => {
  it("maps extensions", () => {
    expect(categoryOf(e("a.JPG", { ext: "jpg" }))).toBe("image");
    expect(categoryOf(e("main.rs"))).toBe("code");
    expect(categoryOf(e("x", { isDir: true }))).toBe("folder");
    expect(categoryOf(e("Dockerfile"))).toBe("code");
    expect(kindLabel(e("thing.xyz"))).toBe("XYZ file");
    expect(kindLabel(e("doc.pdf"))).toBe("PDF document");
  });
});

describe("sortEntries", () => {
  const list = [e("file10.txt", { size: 5 }), e("file2.txt", { size: 50 }), e("Zeta", { isDir: true }), e("alpha", { isDir: true })];
  it("puts folders first and sorts naturally", () => {
    expect(sortEntries(list, { key: "name", dir: "asc" }).map((x) => x.name)).toEqual(["alpha", "Zeta", "file2.txt", "file10.txt"]);
  });
  it("sorts by size descending but keeps folders first", () => {
    expect(sortEntries(list, { key: "size", dir: "desc" }).map((x) => x.name)).toEqual(["Zeta", "alpha", "file2.txt", "file10.txt"]);
  });
});

describe("names", () => {
  it("proposes fresh names", () => {
    expect(freshName("untitled folder", new Set())).toBe("untitled folder");
    expect(freshName("untitled folder", new Set(["untitled folder", "untitled folder 2"]))).toBe("untitled folder 3");
  });
  it("selects the stem when renaming", () => {
    expect(stemRange("photo.final.jpg", false)).toEqual([0, 11]);
    expect(stemRange(".env", false)).toEqual([0, 4]);
    expect(stemRange("my.folder", true)).toEqual([0, 9]);
  });
});

import { volumeFor } from "../components/StatusBar";

describe("volumeFor", () => {
  const vols = [{ mount: "/" }, { mount: "/mnt/data" }, { mount: "C:\\" }, { mount: "D:\\" }];
  it("needs a separator boundary", () => {
    expect(volumeFor(vols, "/mnt/data2/x", "linux")?.mount).toBe("/");
    expect(volumeFor(vols, "/mnt/data/x", "linux")?.mount).toBe("/mnt/data");
    expect(volumeFor(vols, "/mnt/data", "linux")?.mount).toBe("/mnt/data");
  });
  it("matches Windows drives case-insensitively", () => {
    expect(volumeFor(vols, "d:\\Projects", "windows")?.mount).toBe("D:\\");
  });
});
