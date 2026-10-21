import { ArrowLeft, Check, Cloud, KeyRound, Loader2, LogIn, Plug, Server, ShieldAlert, Sparkles, Trash2, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useState } from "react";
import { api, errorMessage } from "../lib/api";
import { DRIVE_COLORS, INTEGRATIONS, S3_PRESETS, SUGGESTED, integration, missingFields, visible, type Field } from "../lib/integrations";
import { openConnection } from "../lib/ops";
import type { ConnKind, Connection } from "../lib/types";
import { useDrives } from "../store/live";
import { confirmDialog, toast, useUi } from "../store/ui";
import { KIND_META } from "./Sidebar";

function KindBadge({ kind, color, size = 36 }: { kind: ConnKind; color: string; size?: number }) {
  const Icon = kind === "sftp" || kind === "ftp" ? Server : Cloud;
  return (
    <span className="flex shrink-0 items-center justify-center rounded-xl" style={{ width: size, height: size, background: `color-mix(in oklab, ${color} 15%, transparent)`, color }}>
      <Icon size={size * 0.46} />
    </span>
  );
}

function Gallery({ onPick }: { onPick: (k: ConnKind) => void }) {
  const connections = useDrives((s) => s.connections);
  return (
    <div className="scroll h-full overflow-y-auto p-6">
      {connections.length > 0 && (
        <section className="mb-7">
          <h3 className="mb-2 text-[11px] font-semibold tracking-[0.08em] text-ink-4 uppercase">Your drives</h3>
          <div className="space-y-1.5">
            {connections.map((c) => (
              <button
                key={c.id}
                onClick={() => useUi.getState().set({ editing: { id: c.id } })}
                className="flex w-full items-center gap-3 rounded-xl border border-line bg-surface p-2.5 text-left hover:bg-surface-2"
              >
                <KindBadge kind={c.kind} color={c.color ?? integration(c.kind).color} size={32} />
                <span className="min-w-0 flex-1">
                  <span className="block truncate font-medium">{c.name}</span>
                  <span className="block truncate text-2xs text-ink-3">
                    {KIND_META[c.kind].label} · {c.options.host || c.options.bucket || c.options.url || "Google account"}
                  </span>
                </span>
                {c.insecureSecrets && (
                  <span title="The system keychain wasn't available, so credentials are stored in a protected file.">
                    <ShieldAlert size={15} className="text-warning" />
                  </span>
                )}
                <span className={`rounded-full px-2 py-0.5 text-[10.5px] font-medium ${c.status === "connected" ? "bg-[color-mix(in_oklab,var(--success)_14%,transparent)] text-success" : c.status === "error" ? "bg-[color-mix(in_oklab,var(--danger)_14%,transparent)] text-danger" : "bg-surface-3 text-ink-3"}`}>
                  {c.status === "connected" ? "Connected" : c.status === "error" ? "Error" : c.status === "connecting" ? "Connecting…" : "Not connected"}
                </span>
              </button>
            ))}
          </div>
        </section>
      )}

      <section>
        <h3 className="mb-2 text-[11px] font-semibold tracking-[0.08em] text-ink-4 uppercase">Add a drive</h3>
        <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-3">
          {INTEGRATIONS.map((i, idx) => (
            <motion.button
              key={i.kind}
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: idx * 0.035, type: "spring", stiffness: 500, damping: 36 }}
              whileHover={{ y: -2 }}
              whileTap={{ scale: 0.98 }}
              onClick={() => onPick(i.kind)}
              className="group flex flex-col items-start gap-3 rounded-2xl border border-line bg-surface p-4 text-left transition-shadow hover:shadow-pop"
            >
              <KindBadge kind={i.kind} color={i.color} />
              <span>
                <span className="block text-[13.5px] font-semibold tracking-tight">{i.name}</span>
                <span className="mt-0.5 block text-2xs leading-snug text-ink-3">{i.tagline}</span>
              </span>
            </motion.button>
          ))}
        </div>
      </section>

      <section className="mt-7">
        <h3 className="mb-1 flex items-center gap-1.5 text-[11px] font-semibold tracking-[0.08em] text-ink-4 uppercase">
          <Sparkles size={12} /> On the roadmap
        </h3>
        <p className="mb-3 text-2xs text-ink-3">Integrations planned next. Several work today through the S3 or WebDAV options above.</p>
        <div className="flex flex-wrap gap-1.5">
          {SUGGESTED.map((s) => (
            <span key={s.name} className="rounded-full border border-dashed border-line-strong px-2.5 py-1 text-2xs text-ink-2" title={s.note}>
              {s.name}
            </span>
          ))}
        </div>
      </section>
    </div>
  );
}

function FieldInput({ f, value, onChange, stored }: { f: Field; value: string; onChange: (v: string) => void; stored: boolean }) {
  const base = "w-full rounded-lg border border-line bg-surface-2/70 px-2.5 text-[12.5px] outline-none transition-colors focus:border-accent focus:bg-surface placeholder:text-ink-4";
  const placeholder = stored ? "•••••••• saved — type to replace" : f.placeholder;
  if (f.type === "toggle")
    return (
      <button type="button" role="switch" aria-checked={value === "true"} onClick={() => onChange(value === "true" ? "false" : "true")} className="flex h-[32px] items-center gap-2.5">
        <span className={`relative h-[18px] w-[32px] rounded-full transition-colors ${value === "true" ? "bg-accent" : "bg-line-strong"}`}>
          <motion.span className="absolute top-[2px] size-[14px] rounded-full bg-white shadow" animate={{ left: value === "true" ? 16 : 2 }} transition={{ type: "spring", stiffness: 700, damping: 35 }} />
        </span>
        <span className="text-[12.5px] text-ink-2">{value === "true" ? "On" : "Off"}</span>
      </button>
    );
  if (f.type === "select")
    return (
      <select value={value} onChange={(e) => onChange(e.target.value)} className={`${base} h-[32px]`}>
        {f.options!.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    );
  if (f.type === "textarea") return <textarea value={value} onChange={(e) => onChange(e.target.value)} placeholder={placeholder ?? "-----BEGIN OPENSSH PRIVATE KEY-----"} rows={4} spellCheck={false} className={`${base} num resize-none py-2 text-[11px]`} />;
  return (
    <input
      type={f.type === "password" ? "password" : f.type === "number" ? "number" : "text"}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      spellCheck={false}
      autoComplete="off"
      className={`${base} h-[32px]`}
    />
  );
}

function Editor({ existing, kind, onBack }: { existing?: Connection; kind: ConnKind; onBack: () => void }) {
  const info = integration(kind);
  const [name, setName] = useState(existing?.name ?? info.name);
  const [color, setColor] = useState(existing?.color ?? info.color);
  const [values, setValues] = useState<Record<string, string>>(() => ({
    ...(kind === "s3" ? { preset: "aws", region: "us-east-1" } : {}),
    ...(kind === "sftp" ? { authMethod: "password" } : {}),
    ...(existing?.options ?? {}),
  }));
  const [busy, setBusy] = useState<null | "test" | "save" | "google">(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const stored = existing?.storedSecrets ?? [];
  const missing = missingFields(kind, values, stored, name);

  const set = (k: string, v: string) => {
    setResult(null);
    setValues((cur) => {
      const next = { ...cur, [k]: v };
      if (kind === "s3" && k === "preset") {
        const p = S3_PRESETS[v];
        next.endpoint = p.endpoint;
        next.region = p.region;
        next.pathStyle = String(p.pathStyle);
      }
      return next;
    });
  };
  const input = () => ({ id: existing?.id ?? null, name, kind, options: values, color });

  const test = async () => {
    setBusy("test");
    setResult(null);
    try {
      const n = await api().testConnection(input());
      setResult({ ok: true, text: `Connected — found ${n} item${n === 1 ? "" : "s"} at the start folder.` });
    } catch (e) {
      setResult({ ok: false, text: errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };

  const save = async (andOpen: boolean) => {
    setBusy("save");
    try {
      const c = await api().saveConnection(input());
      await useDrives.getState().refreshConnections();
      toast({ tone: "success", title: existing ? `Saved ${c.name}` : `Added ${c.name}`, body: c.insecureSecrets ? "The system keychain isn't available; credentials are in a protected file." : undefined });
      if (andOpen) {
        useUi.getState().set({ panel: null, editing: null });
        void openConnection(c.id);
      } else onBack();
    } catch (e) {
      setResult({ ok: false, text: errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };

  const google = async () => {
    setBusy("google");
    setResult(null);
    try {
      const c = await api().saveConnection(input());
      await api().gdriveSignIn(c.id);
      await useDrives.getState().refreshConnections();
      setResult({ ok: true, text: "Signed in with Google." });
      useUi.getState().set({ editing: { id: c.id } });
    } catch (e) {
      setResult({ ok: false, text: errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    if (!existing) return;
    const ok = await confirmDialog({ title: `Remove “${existing.name}”?`, body: "5files forgets this connection and its saved credentials. Files on the server aren't touched.", confirm: "Remove", danger: true });
    if (!ok) return;
    await api().deleteConnection(existing.id);
    await useDrives.getState().refreshConnections();
    onBack();
  };

  const fields = info.fields.filter((f) => visible(f, values));
  const needsGoogle = kind === "gdrive" && !stored.includes("refreshToken");

  return (
    <div className="flex h-full flex-col">
      <div className="scroll flex-1 overflow-y-auto px-6 pt-5 pb-6">
        <div className="mb-5 flex items-center gap-3">
          <KindBadge kind={kind} color={color} size={40} />
          <div className="min-w-0 flex-1">
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="w-full rounded-md bg-transparent text-[16px] font-semibold tracking-tight outline-none focus:bg-surface-2"
              aria-label="Drive name"
              placeholder="Drive name"
            />
            <p className="text-2xs text-ink-3">{info.tagline}</p>
          </div>
          <div className="flex gap-1">
            {DRIVE_COLORS.map((c) => (
              <button key={c} onClick={() => setColor(c)} className="flex size-5 items-center justify-center rounded-full" style={{ background: c }} aria-label={`Color ${c}`}>
                {color === c && <Check size={11} className="text-white" strokeWidth={3} />}
              </button>
            ))}
          </div>
        </div>

        <div className="grid grid-cols-2 gap-x-3 gap-y-3.5">
          {fields.map((f) => (
            <label key={f.key} className={f.half ? "col-span-1" : "col-span-2"}>
              <span className="mb-1 flex items-center gap-1 text-[11.5px] font-medium text-ink-2">
                {f.label}
                {f.required && <span className="text-ink-4">*</span>}
                {f.secret && <KeyRound size={10} className="text-ink-4" />}
              </span>
              <FieldInput f={f} value={values[f.key] ?? ""} onChange={(v) => set(f.key, v)} stored={!!f.secret && stored.includes(f.key)} />
              {f.help && <span className="mt-1 block text-[11px] leading-snug text-ink-3">{f.help}</span>}
            </label>
          ))}
        </div>

        {kind === "gdrive" && (
          <div className="mt-5 rounded-xl border border-line bg-surface-2 p-3.5">
            <p className="text-[12.5px] font-medium">{needsGoogle ? "Sign in to give 5files access to your Drive" : "Signed in with Google"}</p>
            <p className="mt-0.5 text-2xs text-ink-3">Your browser opens Google's sign-in page. The access token is kept in your system keychain.</p>
            <button
              onClick={google}
              disabled={!!busy || missing.length > 0}
              className="mt-3 flex h-8 items-center gap-2 rounded-lg bg-ink px-3 text-[12.5px] font-medium text-surface hover:opacity-90 disabled:opacity-40"
            >
              {busy === "google" ? <Loader2 size={14} className="animate-spin" /> : <LogIn size={14} />}
              {busy === "google" ? "Waiting for Google…" : needsGoogle ? "Sign in with Google" : "Sign in again"}
            </button>
          </div>
        )}

        <AnimatePresence>
          {result && (
            <motion.div
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: "auto" }}
              exit={{ opacity: 0, height: 0 }}
              className={`mt-4 overflow-hidden rounded-xl border px-3 py-2.5 text-[12px] ${result.ok ? "border-[color-mix(in_oklab,var(--success)_35%,transparent)] bg-[color-mix(in_oklab,var(--success)_8%,transparent)] text-success" : "border-[color-mix(in_oklab,var(--danger)_35%,transparent)] bg-[color-mix(in_oklab,var(--danger)_8%,transparent)] text-danger"}`}
            >
              <span className="break-words">{result.text}</span>
            </motion.div>
          )}
        </AnimatePresence>
      </div>

      <div className="flex shrink-0 items-center gap-2 border-t border-line px-6 py-3">
        {existing && (
          <button onClick={remove} className="flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-[12.5px] font-medium text-danger hover:bg-[color-mix(in_oklab,var(--danger)_10%,transparent)]">
            <Trash2 size={13} /> Remove
          </button>
        )}
        <span className="flex-1 truncate text-2xs text-ink-4">{missing.length > 0 && `Needs: ${missing.join(", ")}`}</span>
        <button
          onClick={test}
          disabled={!!busy || missing.length > 0 || needsGoogle}
          className="flex h-8 items-center gap-1.5 rounded-lg border border-line px-3 text-[12.5px] font-medium hover:bg-surface-2 disabled:opacity-40"
        >
          {busy === "test" ? <Loader2 size={13} className="animate-spin" /> : <Plug size={13} />} Test
        </button>
        <button onClick={() => save(false)} disabled={!!busy || missing.length > 0} className="h-8 rounded-lg border border-line px-3 text-[12.5px] font-medium hover:bg-surface-2 disabled:opacity-40">
          Save
        </button>
        <button
          onClick={() => save(true)}
          disabled={!!busy || missing.length > 0 || needsGoogle}
          className="flex h-8 items-center gap-1.5 rounded-lg bg-accent px-3.5 text-[12.5px] font-medium text-white hover:brightness-110 disabled:opacity-40"
        >
          {busy === "save" && <Loader2 size={13} className="animate-spin" />} Save & open
        </button>
      </div>
    </div>
  );
}

export function ConnectionsPanel() {
  const editing = useUi((s) => s.editing);
  const connections = useDrives((s) => s.connections);
  const existing = useMemo(() => (editing?.id ? connections.find((c) => c.id === editing.id) : undefined), [editing, connections]);
  const kind = existing?.kind ?? editing?.kind;
  useEffect(() => {
    void useDrives.getState().refreshConnections();
  }, []);
  const back = () => useUi.getState().set({ editing: null });
  const close = () => useUi.getState().set({ panel: null, editing: null });

  return (
    <div className="flex h-[min(640px,85vh)] flex-col">
      <div className="flex h-14 shrink-0 items-center gap-2 border-b border-line px-4">
        {kind && (
          <button onClick={back} className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3 hover:text-ink" aria-label="Back">
            <ArrowLeft size={16} />
          </button>
        )}
        <div className="flex-1 pl-1">
          <h2 className="text-[15px] font-semibold tracking-tight">{kind ? (existing ? "Edit drive" : `Connect ${integration(kind).name}`) : "Remote drives"}</h2>
          {!kind && <p className="text-2xs text-ink-3">Cloud storage and servers show up in the sidebar, like any other disk.</p>}
        </div>
        <button onClick={close} className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-surface-3 hover:text-ink" aria-label="Close">
          <X size={16} />
        </button>
      </div>
      <div className="relative min-h-0 flex-1">
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.div
            key={kind ? (existing?.id ?? `new-${kind}`) : "gallery"}
            initial={{ opacity: 0, x: kind ? 24 : -24 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: kind ? -24 : 24 }}
            transition={{ type: "spring", stiffness: 500, damping: 40 }}
            className="absolute inset-0"
          >
            {kind ? <Editor existing={existing} kind={kind} onBack={back} /> : <Gallery onPick={(k) => useUi.getState().set({ editing: { kind: k } })} />}
          </motion.div>
        </AnimatePresence>
      </div>
    </div>
  );
}
