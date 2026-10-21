import type { ConnKind } from "./types";

export interface Field {
  key: string;
  label: string;
  type: "text" | "password" | "number" | "toggle" | "select" | "textarea";
  placeholder?: string;
  required?: boolean;
  help?: string;
  options?: { value: string; label: string }[];
  /** Stored in the OS keychain, never shown again. */
  secret?: boolean;
  half?: boolean;
  /** Show only when another field has a value. */
  when?: { key: string; is: string[] };
}

export interface Integration {
  kind: ConnKind;
  name: string;
  tagline: string;
  color: string;
  fields: Field[];
}

export const S3_PRESETS: Record<string, { label: string; endpoint: string; region: string; pathStyle: boolean }> = {
  aws: { label: "Amazon S3", endpoint: "", region: "us-east-1", pathStyle: false },
  r2: { label: "Cloudflare R2", endpoint: "https://<account-id>.r2.cloudflarestorage.com", region: "auto", pathStyle: true },
  minio: { label: "MinIO", endpoint: "http://localhost:9000", region: "us-east-1", pathStyle: true },
  wasabi: { label: "Wasabi", endpoint: "https://s3.wasabisys.com", region: "us-east-1", pathStyle: false },
  b2: { label: "Backblaze B2", endpoint: "https://s3.us-west-004.backblazeb2.com", region: "us-west-004", pathStyle: false },
  spaces: { label: "DigitalOcean Spaces", endpoint: "https://nyc3.digitaloceanspaces.com", region: "nyc3", pathStyle: false },
  custom: { label: "Other S3-compatible", endpoint: "", region: "us-east-1", pathStyle: true },
};

export const INTEGRATIONS: Integration[] = [
  {
    kind: "s3",
    name: "S3",
    tagline: "AWS, Cloudflare R2, MinIO, Wasabi, Backblaze B2…",
    color: "#f59e0b",
    fields: [
      { key: "preset", label: "Provider", type: "select", options: Object.entries(S3_PRESETS).map(([value, p]) => ({ value, label: p.label })) },
      { key: "bucket", label: "Bucket", type: "text", required: true, placeholder: "my-bucket", half: true },
      { key: "region", label: "Region", type: "text", placeholder: "us-east-1", half: true },
      { key: "endpoint", label: "Endpoint", type: "text", placeholder: "Leave empty for AWS", help: "Required for S3-compatible services." },
      { key: "accessKeyId", label: "Access key ID", type: "text", required: true, half: true },
      { key: "secretAccessKey", label: "Secret access key", type: "password", secret: true, required: true, half: true },
      { key: "root", label: "Start folder", type: "text", placeholder: "/", half: true },
      { key: "pathStyle", label: "Path-style URLs", type: "toggle", half: true, help: "Needed by MinIO and most self-hosted servers." },
    ],
  },
  {
    kind: "gdrive",
    name: "Google Drive",
    tagline: "Your Drive, signed in with Google",
    color: "#1a73e8",
    fields: [
      {
        key: "clientId",
        label: "OAuth client ID",
        type: "text",
        required: true,
        placeholder: "xxxx.apps.googleusercontent.com",
        help: "Create a “Desktop app” OAuth client in Google Cloud Console → APIs & Services → Credentials, with the Drive API enabled.",
      },
      { key: "clientSecret", label: "Client secret", type: "password", secret: true, required: true },
      { key: "root", label: "Start folder", type: "text", placeholder: "/" },
    ],
  },
  {
    kind: "nextcloud",
    name: "Nextcloud",
    tagline: "Nextcloud and ownCloud servers",
    color: "#0082c9",
    fields: [
      { key: "url", label: "Server address", type: "text", required: true, placeholder: "https://cloud.example.com" },
      { key: "username", label: "Username", type: "text", required: true, half: true },
      { key: "password", label: "App password", type: "password", secret: true, required: true, half: true, help: "Settings → Security → Devices & sessions → Create new app password." },
      { key: "root", label: "Start folder", type: "text", placeholder: "/" },
    ],
  },
  {
    kind: "webdav",
    name: "WebDAV",
    tagline: "Synology, QNAP, Box, any WebDAV server",
    color: "#6366f1",
    fields: [
      { key: "url", label: "WebDAV URL", type: "text", required: true, placeholder: "https://dav.example.com/files" },
      { key: "username", label: "Username", type: "text", half: true },
      { key: "password", label: "Password", type: "password", secret: true, half: true },
      { key: "root", label: "Start folder", type: "text", placeholder: "/" },
    ],
  },
  {
    kind: "sftp",
    name: "SSH / SFTP",
    tagline: "Any server you can SSH into",
    color: "#10b981",
    fields: [
      { key: "host", label: "Host", type: "text", required: true, placeholder: "server.example.com", half: true },
      { key: "port", label: "Port", type: "number", placeholder: "22", half: true },
      { key: "username", label: "Username", type: "text", required: true },
      {
        key: "authMethod",
        label: "Sign in with",
        type: "select",
        options: [
          { value: "password", label: "Password" },
          { value: "key", label: "Private key" },
        ],
      },
      { key: "password", label: "Password", type: "password", secret: true, when: { key: "authMethod", is: ["password", ""] } },
      { key: "keyPath", label: "Private key file", type: "text", placeholder: "~/.ssh/id_ed25519", when: { key: "authMethod", is: ["key"] }, help: "Or paste the key below instead." },
      { key: "privateKey", label: "Private key (paste)", type: "textarea", secret: true, when: { key: "authMethod", is: ["key"] } },
      { key: "passphrase", label: "Key passphrase", type: "password", secret: true, when: { key: "authMethod", is: ["key"] } },
      { key: "root", label: "Start folder", type: "text", placeholder: "Home folder" },
    ],
  },
  {
    kind: "ftp",
    name: "FTP",
    tagline: "FTP and FTPS servers",
    color: "#ef4444",
    fields: [
      { key: "host", label: "Host", type: "text", required: true, placeholder: "ftp.example.com", half: true },
      { key: "port", label: "Port", type: "number", placeholder: "21", half: true },
      { key: "username", label: "Username", type: "text", half: true, placeholder: "anonymous" },
      { key: "password", label: "Password", type: "password", secret: true, half: true },
      { key: "secure", label: "Use FTPS (TLS)", type: "toggle" },
      { key: "root", label: "Start folder", type: "text", placeholder: "/" },
    ],
  },
];

export const integration = (k: ConnKind) => INTEGRATIONS.find((i) => i.kind === k)!;

/** Next integrations worth building, roughly by demand. */
export const SUGGESTED: { name: string; note: string }[] = [
  { name: "Dropbox", note: "OAuth, via OpenDAL" },
  { name: "OneDrive / SharePoint", note: "Microsoft Graph" },
  { name: "SMB / Windows shares", note: "NAS & office networks" },
  { name: "Azure Blob Storage", note: "via OpenDAL" },
  { name: "Google Cloud Storage", note: "via OpenDAL" },
  { name: "Box", note: "OAuth" },
  { name: "iCloud Drive", note: "local sync folder on macOS" },
  { name: "pCloud · MEGA · Proton Drive", note: "consumer clouds" },
  { name: "Git repositories", note: "browse any branch or commit" },
  { name: "Docker volumes & containers", note: "via the Docker API" },
  { name: "Kubernetes PVCs", note: "kubectl exec / cp" },
  { name: "IPFS · HDFS", note: "via OpenDAL" },
];

export const DRIVE_COLORS = ["#3b4cf5", "#10b981", "#f59e0b", "#ef4444", "#a23ad9", "#0ea5e9", "#64748b"];

/** Client-side validation mirroring the backend's required fields. */
export function missingFields(kind: ConnKind, values: Record<string, string>, stored: string[], name: string): string[] {
  const missing: string[] = [];
  if (!name.trim()) missing.push("Name");
  for (const f of integration(kind).fields) {
    if (!f.required || !visible(f, values)) continue;
    const v = (values[f.key] ?? "").trim();
    if (!v && !(f.secret && stored.includes(f.key))) missing.push(f.label);
  }
  return missing;
}

export function visible(f: Field, values: Record<string, string>) {
  if (!f.when) return true;
  return f.when.is.includes(values[f.when.key] ?? "");
}
