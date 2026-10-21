import { describe, expect, it } from "vitest";
import { INTEGRATIONS, integration, missingFields, visible } from "./integrations";

describe("integrations", () => {
  it("covers the six launch integrations", () => {
    expect(INTEGRATIONS.map((i) => i.kind).sort()).toEqual(["ftp", "gdrive", "nextcloud", "s3", "sftp", "webdav"]);
  });

  it("flags required fields, accepting stored secrets", () => {
    expect(missingFields("s3", { bucket: "b", accessKeyId: "k" }, [], "My S3")).toEqual(["Secret access key"]);
    expect(missingFields("s3", { bucket: "b", accessKeyId: "k" }, ["secretAccessKey"], "My S3")).toEqual([]);
    expect(missingFields("ftp", { host: "" }, [], "")).toEqual(["Name", "Host"]);
  });

  it("shows SFTP key fields only for key auth", () => {
    const key = integration("sftp").fields.find((f) => f.key === "keyPath")!;
    const pw = integration("sftp").fields.find((f) => f.key === "password")!;
    expect(visible(key, { authMethod: "key" })).toBe(true);
    expect(visible(key, { authMethod: "password" })).toBe(false);
    expect(visible(pw, {})).toBe(true);
  });

  it("marks every secret field as a password/textarea so it is never echoed", () => {
    for (const i of INTEGRATIONS) for (const f of i.fields) if (f.secret) expect(["password", "textarea"]).toContain(f.type);
  });
});
