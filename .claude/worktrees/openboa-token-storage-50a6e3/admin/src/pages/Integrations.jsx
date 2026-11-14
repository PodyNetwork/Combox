// Connect the company's own tools. Keys are encrypted at rest and never
// shown again after saving: only the last four characters. On the platform
// they are stored per company; in a single-company deployment they are
// stored in its own database and replace the server's .env values.
//
// The page is one list: each integration is a row with its status, and
// opens a dialog where all of its settings are edited and saved at once.
import { useCallback, useEffect, useId, useMemo, useState } from "react";
import { platform } from "../api.js";
import { Badge, Button, ErrorNote, Icon, Loading, Modal, PageHead, Panel, Segmented, useApp, useCompanyApi } from "../ui.jsx";

// Icons the shared set doesn't have, keyed by integration id.
const GLYPHS = {
  email: "M3 6h18v12H3zM3 7l9 6 9-6",
  docuseal: "M14 3H6v18h12V7zM14 3v4h4M9 13h6M9 17h4",
  crm: "M4 5h16v11H8l-4 4zM8 9h8M8 12h5",
  hiring: "M3 8h18v12H3zM8 8V5h8v3M3 13h18",
  ai: "M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8zM19 16l.8 2.2L22 19l-2.2.8L19 22l-.8-2.2L16 19l2.2-.8z",
  mcp: "M15 7a4 4 0 1 1-3.5 6L4 20.5V17h3v-3h3l1.5-1.5A4 4 0 0 1 15 7z",
};
const EYE = "M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z";
const EYE_OFF = "M3 3l18 18M10.6 5.1A10 10 0 0 1 12 5c6.5 0 10 7 10 7a17 17 0 0 1-3.2 4M6.6 6.6A17 17 0 0 0 2 12s3.5 7 10 7a9.6 9.6 0 0 0 5.4-1.6M9.9 9.9a3 3 0 0 0 4.2 4.2";
const SPARK = "M12 4v4M12 16v4M4 12h4M16 12h4M6.3 6.3l2.8 2.8M14.9 14.9l2.8 2.8M6.3 17.7l2.8-2.8M14.9 9.1l2.8-2.8";
const Glyph = ({ d, size = 16, className = "" }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" className={className} aria-hidden="true">
    <path d={d} />
  </svg>
);

const FIELD = "w-full rounded-xl border border-white/10 bg-black/40 px-3.5 py-2.5 text-sm text-fog placeholder:text-dim transition focus:border-lime/60 focus:outline-none focus:ring-4 focus:ring-lime/10 disabled:opacity-50";

// The marketplace has two values and defaults to the jobs board; the Upwork
// settings only matter when Upwork is picked.
const MARKETPLACE = "AXOM_MARKETPLACE";
const UPWORK = ["UPWORK_MCP_URL", "UPWORK_MCP_TOKEN"];
const MCP_TOKEN = "AXOM_MCP_TOKEN";

// "DocuSeal (contracts and tax forms)" -> ["DocuSeal", "contracts and tax forms"].
const split = (s) => {
  const m = /^(.*?)\s*\((.*)\)\s*$/.exec(s);
  return m ? [m[1], m[2]] : [s, ""];
};
const sentence = (s) => (s ? s[0].toUpperCase() + s.slice(1) : s);

const newToken = () => {
  const b = new Uint8Array(32);
  crypto.getRandomValues(b);
  return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
};

/** What a company has set for one integration, and whether that's enough. */
function statusOf(integration, values, server, shared) {
  const has = (n) => Boolean(values[n] || server.includes(n));
  const market = values[MARKETPLACE]?.value || "board";
  const needed = integration.fields.filter((f) => {
    if (/\(optional\)/i.test(f.label)) return false;
    if (f.name === MARKETPLACE) return false;
    if (UPWORK.includes(f.name)) return market === "upwork";
    return true;
  });
  const set = integration.fields.filter((f) => has(f.name)).length;
  const done = needed.filter((f) => has(f.name)).length;
  const fromServer = integration.fields.some((f) => !values[f.name] && server.includes(f.name));
  if (done === needed.length && (needed.length || integration.fields.some((f) => f.name === MARKETPLACE))) {
    const label = integration.fields.some((f) => f.name === MARKETPLACE)
      ? market === "upwork" ? "Upwork" : "ForeMan Jobs board"
      : fromServer ? "Connected · server" : "Connected";
    return { tone: "lime", label, connected: true, set };
  }
  if (set === 0 && integration.fields.every((f) => shared.includes(f.name))) return { tone: "sky", label: "ForeMan shared", set };
  if (done > 0) return { tone: "amber", label: `${done} of ${needed.length} set`, set, progress: done / needed.length };
  return { tone: "mute", label: "Not connected", set };
}

export default function Integrations() {
  const { session, slug, notify, platform: isPlatform, navigate } = useApp();
  const companyApi = useCompanyApi();
  const api = useCallback(
    (path, opts) => (isPlatform ? platform(session, `/companies/${slug}/integrations${path}`, opts) : companyApi(`/admin/integrations${path}`, opts)),
    [isPlatform, session, slug, companyApi],
  );
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  const [open, setOpen] = useState(null);
  const reload = useCallback(() => api("").then(setData, (e) => setError(e.message)), [api]);
  useEffect(() => {
    reload();
  }, [reload]);

  const values = useMemo(() => Object.fromEntries((data?.values || []).map((v) => [v.name, v])), [data]);
  const server = data?.server || [];
  const shared = data?.shared || [];
  const rows = (data?.integrations || []).map((i) => ({ integration: i, status: statusOf(i, values, server, shared) }));
  const connected = rows.filter((r) => r.status.connected).length;
  const locked = data?.can_store === false;
  const current = rows.find((r) => r.integration.id === open);

  return (
    <div className="max-w-4xl">
      <PageHead kicker={data ? `${connected} of ${rows.length} connected` : "Your own accounts"} title="Integrations">
        {data && (
          <span title={data.live ? "Connected integrations make real calls: emails, contracts, posts." : "Until AXOM_LIVE=true is set on the server, ForeMan records what it would send without calling the service."}>
            {data.live ? <Badge tone="lime" dot>Live</Badge> : <Badge tone="amber">Dry-run</Badge>}
          </span>
        )}
      </PageHead>
      <p className="reveal -mt-2 mb-6 max-w-2xl text-sm text-mute">
        Connect your own accounts. Keys are encrypted, and only their last four characters are ever shown again.
        {data && !data.live && " In dry-run, ForeMan records what it would send without calling the service."} Tools for AI workers are MCP servers, on the{" "}
        <button type="button" onClick={() => navigate("tools")} className="text-fog underline decoration-white/30 underline-offset-4 hover:decoration-lime">
          Tools page
        </button>
        .
      </p>

      {locked && (
        <div className="reveal mb-4 flex items-start gap-3 rounded-2xl bg-ember/5 px-4 py-3 text-sm ring-1 ring-ember/25">
          <Icon name="key" size={16} className="mt-0.5 shrink-0 text-ember" />
          <p className="text-mute">
            <span className="text-fog">Read-only.</span> {data.reason}
          </p>
        </div>
      )}
      <ErrorNote error={error} />
      {!data && !error && <Loading rows={6} />}

      {data && (
        <Panel as="ul" className="divide-y divide-white/5 p-1.5">
          {rows.map(({ integration, status }) => (
            <Row key={integration.id} integration={integration} status={status} onOpen={() => setOpen(integration.id)} />
          ))}
        </Panel>
      )}

      {current && (
        <Editor
          key={current.integration.id}
          integration={current.integration}
          status={current.status}
          values={values}
          server={server}
          locked={locked}
          reason={data?.reason}
          api={api}
          notify={notify}
          onClose={() => setOpen(null)}
          onSaved={reload}
        />
      )}
    </div>
  );
}

function StatusBadge({ status }) {
  return (
    <Badge tone={status.tone}>
      {status.connected && <Icon name="check" size={12} />}
      {status.label}
    </Badge>
  );
}

function Row({ integration, status, onOpen }) {
  const [name, tagline] = split(integration.name);
  const dot = status.connected ? "bg-lime" : status.tone === "amber" ? "bg-amber" : status.tone === "sky" ? "bg-sky" : "bg-white/25";
  return (
    <li>
      <button
        type="button"
        onClick={onOpen}
        className="group flex w-full items-center gap-4 rounded-[1.1rem] px-3.5 py-3.5 text-left transition hover:bg-white/4 focus-visible:bg-white/4 sm:px-4"
      >
        <span
          className={`flex size-10 shrink-0 items-center justify-center rounded-xl ring-1 ring-inset transition ${status.connected ? "bg-lime/10 text-lime ring-lime/25" : "bg-white/5 text-mute ring-white/10 group-hover:text-fog"}`}
        >
          <Glyph d={GLYPHS[integration.id] || GLYPHS.mcp} size={18} />
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex flex-wrap items-baseline gap-x-2">
            <span className="font-medium text-fog">{name}</span>
            {tagline && <span className="text-xs text-dim">{sentence(tagline)}</span>}
          </span>
          <span className="mt-0.5 block truncate text-xs text-mute" title={integration.description}>{integration.description}</span>
          {status.progress != null && (
            <span className="mt-2 block h-1 w-28 overflow-hidden rounded-full bg-white/10" aria-hidden="true">
              <span className="block h-full rounded-full bg-amber" style={{ width: `${Math.round(status.progress * 100)}%` }} />
            </span>
          )}
        </span>
        <span className="hidden sm:block">
          <StatusBadge status={status} />
        </span>
        <span className="flex items-center gap-1.5 text-sm text-mute transition group-hover:text-fog">
          <span className={`size-2 rounded-full sm:hidden ${dot}`} aria-label={status.label} />
          <span className="hidden md:inline">{status.set || status.connected ? "Manage" : "Connect"}</span>
          <Icon name="chevronRight" size={16} className="transition group-hover:translate-x-0.5" />
        </span>
      </button>
    </li>
  );
}

// Edits every setting of one integration and saves them together.
function Editor({ integration, status, values, server, locked, reason, api, notify, onClose, onSaved }) {
  const [name, tagline] = split(integration.name);
  // Plain settings start from their saved value; secrets start empty.
  const initial = useMemo(
    () => Object.fromEntries(integration.fields.map((f) => [f.name, !f.secret && values[f.name]?.value ? values[f.name].value : ""])),
    [integration, values],
  );
  const [draft, setDraft] = useState(initial);
  const [removed, setRemoved] = useState(() => new Set());
  const [errors, setErrors] = useState({});
  const [busy, setBusy] = useState(false);
  const [confirmAll, setConfirmAll] = useState(false);

  const market = draft[MARKETPLACE] || values[MARKETPLACE]?.value || "board";
  const visible = integration.fields.filter((f) => !UPWORK.includes(f.name) || market === "upwork");
  const stored = integration.fields.filter((f) => values[f.name]);

  // What Save will do: a changed value is stored; a removed secret or a
  // cleared plain setting is deleted.
  const puts = visible.filter((f) => draft[f.name].trim() && draft[f.name].trim() !== initial[f.name] && !removed.has(f.name));
  const dels = stored.filter((f) => removed.has(f.name) || (!f.secret && f.name !== MARKETPLACE && !draft[f.name].trim()));
  const dirty = puts.length + dels.length > 0;

  const set = (n, v) => {
    setDraft((d) => ({ ...d, [n]: v }));
    setErrors((e) => ({ ...e, [n]: undefined }));
  };
  const toggleRemove = (n) =>
    setRemoved((r) => {
      const next = new Set(r);
      if (next.has(n)) next.delete(n);
      else next.add(n);
      return next;
    });

  // Runs each call, keeps going past failures, and returns them by field.
  const run = async (ops) => {
    setBusy(true);
    const failed = {};
    for (const [f, op] of ops) {
      try {
        await op();
      } catch (e) {
        failed[f.name] = sentence(e.message);
      }
    }
    setBusy(false);
    onSaved();
    return failed;
  };

  const save = async (e) => {
    e?.preventDefault();
    if (!dirty || busy || locked) return;
    const failed = await run([
      ...puts.map((f) => [f, () => api(`/${f.name}`, { method: "PUT", body: { value: draft[f.name].trim() } })]),
      ...dels.map((f) => [f, () => api(`/${f.name}`, { method: "DELETE" })]),
    ]);
    if (Object.keys(failed).length) {
      setErrors(failed);
      notify(`Couldn't save ${name}. See the highlighted fields.`, "error");
      return;
    }
    notify(status.set ? `${name} updated.` : `${name} connected.`);
    onClose();
  };

  const disconnect = async () => {
    const failed = await run(stored.map((f) => [f, () => api(`/${f.name}`, { method: "DELETE" })]));
    if (Object.keys(failed).length) {
      setErrors(failed);
      setConfirmAll(false);
      notify(`Couldn't disconnect ${name}.`, "error");
      return;
    }
    notify(`${name} disconnected.`);
    onClose();
  };

  return (
    <Modal open onClose={onClose} title={name} kicker={tagline || "Integration"} backdropClose={!dirty}>
      <div className="-mt-1 mb-5 space-y-3">
        <StatusBadge status={status} />
        <p className="text-sm text-mute">{integration.description}</p>
      </div>

      {locked && (
        <p className="mb-5 flex items-start gap-2 rounded-xl bg-ember/5 px-3 py-2.5 text-xs text-mute ring-1 ring-ember/25">
          <Icon name="key" size={14} className="mt-px shrink-0 text-ember" /> {reason}
        </p>
      )}

      <form onSubmit={save} className="space-y-4" autoComplete="off">
        {visible.map((f, n) =>
          f.name === MARKETPLACE ? (
            <MarketplaceField key={f.name} value={market} disabled={locked} onChange={(v) => set(f.name, v)} error={errors[f.name]} />
          ) : (
            <SettingField
              key={f.name}
              field={f}
              value={draft[f.name]}
              stored={values[f.name]}
              onServer={server.includes(f.name)}
              removing={removed.has(f.name)}
              disabled={locked}
              autoFocus={n === 0 && !locked}
              error={errors[f.name]}
              onChange={(v) => set(f.name, v)}
              onRemove={() => toggleRemove(f.name)}
            />
          ),
        )}

        <div className="flex flex-wrap items-center gap-2 border-t border-white/5 pt-5">
          {stored.length > 0 && !locked &&
            (confirmAll ? (
              <span className="flex items-center gap-2">
                <Button type="button" size="sm" variant="danger" disabled={busy} onClick={disconnect}>Remove all {stored.length}</Button>
                <Button type="button" size="sm" variant="link" onClick={() => setConfirmAll(false)}>Keep</Button>
              </span>
            ) : (
              <Button type="button" size="sm" variant="link" className="hover:text-ember" onClick={() => setConfirmAll(true)}>
                Disconnect
              </Button>
            ))}
          <span className="ml-auto flex gap-2">
            <Button type="button" variant="ghost" onClick={onClose}>{dirty ? "Cancel" : "Close"}</Button>
            <Button type="submit" disabled={!dirty || busy || locked}>
              {busy ? "Saving…" : status.set || status.connected ? "Save changes" : "Connect"}
            </Button>
          </span>
        </div>
      </form>
    </Modal>
  );
}

function MarketplaceField({ value, onChange, disabled, error }) {
  return (
    <div className={disabled ? "pointer-events-none opacity-50" : ""}>
      <Segmented
        label="Post human jobs on"
        value={value}
        onChange={onChange}
        options={[
          { value: "board", label: "ForeMan Jobs board" },
          { value: "upwork", label: "Upwork" },
        ]}
      />
      <p className="mt-1.5 text-xs text-dim">{value === "upwork" ? "Hires are paid through Upwork only. Add your Upwork MCP server below." : "The default. Hires are paid in NEAR."}</p>
      {error && <p className="mt-1.5 text-xs text-ember">{error}</p>}
    </div>
  );
}

function SettingField({ field, value, stored, onServer, removing, disabled, autoFocus, error, onChange, onRemove }) {
  const id = useId();
  const { notify } = useApp();
  const [label, note] = split(field.label);
  const [show, setShow] = useState(false);
  const token = field.name === MCP_TOKEN;

  const placeholder = removing
    ? "Removed when you save"
    : stored && field.secret
      ? `Saved ${stored.hint} · type to replace`
      : !stored && onServer
        ? "Set in the server's .env · type to override"
        : field.placeholder || (field.secret ? "Paste key" : "");

  return (
    <div className="text-sm">
      <div className="flex items-center gap-2">
        <label htmlFor={id} className="text-fog/90">{label}</label>
        {note && <span className="text-xs text-dim">{sentence(note)}</span>}
        <span className="ml-auto flex items-center gap-2 text-xs">
          {(stored || onServer) && !removing && <Icon name="check" size={14} className="text-lime" />}
          {stored && field.secret && !disabled && (
            <button type="button" onClick={onRemove} className={`transition ${removing ? "text-fog underline underline-offset-4" : "text-mute hover:text-ember"}`}>
              {removing ? "Undo" : "Remove"}
            </button>
          )}
        </span>
      </div>
      <div className="relative mt-1.5">
        <input
          id={id}
          type={field.secret && !show ? "password" : "text"}
          value={removing ? "" : value}
          disabled={disabled || removing}
          autoFocus={autoFocus}
          placeholder={placeholder}
          onChange={(e) => onChange(e.target.value)}
          autoComplete="off"
          spellCheck={false}
          aria-invalid={Boolean(error)}
          aria-describedby={error ? `${id}-err` : undefined}
          className={`${FIELD} font-mono ${field.secret ? (token ? "pr-32" : "pr-11") : ""} ${error ? "border-ember/60" : ""}`}
        />
        {field.secret && (
          <span className="absolute inset-y-0 right-1.5 flex items-center gap-1">
            {token && !disabled && !removing && (
              <button
                type="button"
                onClick={() => {
                  onChange(newToken());
                  setShow(true);
                }}
                className="flex items-center gap-1 rounded-lg px-2 py-1 text-xs text-mute transition hover:bg-white/5 hover:text-lime"
                title="Generate a random 64-character token"
              >
                <Glyph d={SPARK} size={13} /> Generate
              </button>
            )}
            <button
              type="button"
              onClick={() => setShow((s) => !s)}
              disabled={!value || removing}
              aria-label={show ? `Hide ${label}` : `Show ${label}`}
              className="rounded-lg p-1.5 text-dim transition hover:bg-white/5 hover:text-fog disabled:opacity-30"
            >
              <Glyph d={show ? EYE_OFF : EYE} />
            </button>
          </span>
        )}
      </div>
      {error ? (
        <p id={`${id}-err`} className="mt-1.5 text-xs text-ember">{error}</p>
      ) : token && value && show && !removing ? (
        <p className="mt-1.5 flex items-center gap-2 text-xs text-amber">
          <span className="mr-auto">Copy this token now: after saving, only its last four characters are shown.</span>
          <button
            type="button"
            onClick={() => navigator.clipboard?.writeText(value).then(() => notify("Token copied."), () => notify("Copy failed.", "error"))}
            className="flex shrink-0 items-center gap-1 rounded-lg px-2 py-1 text-mute ring-1 ring-white/10 transition hover:text-fog"
          >
            <Icon name="copy" size={12} /> Copy
          </button>
        </p>
      ) : null}
    </div>
  );
}
