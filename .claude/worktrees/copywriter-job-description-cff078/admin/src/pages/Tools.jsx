// Tools AI workers can use: MCP servers (whose tools ForeMan discovers) and
// HTTP webhooks your admins connect. The standard names roles use (crm,
// code_repo, postiz, ...) are filled by connecting a server with that name.
// A worker uses a tool only once it's on the worker's list (Team page).
import { useState } from "react";
import { Badge, Button, Chip, ErrorNote, Eyebrow, Icon, Input, KineticText, Loading, Modal, PageHead, Panel, PanelHead, Segmented, ago, useApp, useCompanyApi, useLoad, when } from "../ui.jsx";

// Icons the shared set doesn't have.
const REFRESH = "M20 11a8 8 0 0 0-14.3-4.9L4 8M4 4v4h4M4 13a8 8 0 0 0 14.3 4.9L20 16M20 20v-4h-4";
const TRASH = "M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3";
const Glyph = ({ d, size = 14 }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d={d} />
  </svg>
);

export default function Tools() {
  const { navigate } = useApp();
  const { data, error, loading, reload } = useLoad("/admin/tools");
  // false, or the starting values of the add form (true = empty).
  const [adding, setAdding] = useState(false);
  const standard = data?.standard || [];
  const connected = standard.filter((t) => t.connected).length;
  return (
    <>
      <PageHead kicker={data ? `${connected} of ${standard.length} standard connected · ${data.tools.length} tools` : "What AI workers can use"} title="Tools">
        {data?.live && <Badge tone="lime" dot>Live: tool calls are made</Badge>}
        <Button onClick={() => setAdding(true)}><Icon name="plus" size={16} /> Add tool</Button>
      </PageHead>
      <p className="reveal -mt-2 mb-6 max-w-2xl text-sm text-mute">
        What your AI workers can use: MCP servers and webhooks you connect. Give tools to workers on the{" "}
        <button type="button" onClick={() => navigate("team")} className="text-fog underline decoration-white/30 underline-offset-4 hover:decoration-lime">Team page</button>.
      </p>
      <ErrorNote error={error} />
      {loading && <Loading rows={4} />}
      {data && (
        <div className="space-y-4">
          {!data.live && (
            <div className="reveal flex items-start gap-3 rounded-2xl bg-amber/5 px-4 py-3 text-sm ring-1 ring-amber/25">
              <Badge tone="amber">Dry-run</Badge>
              <p className="text-mute">Until AXOM_LIVE is turned on (after legal review), tool calls are recorded but not made. Discovering an MCP server's tools still contacts it.</p>
            </div>
          )}
          <div className="grid grid-cols-1 items-start gap-4 lg:grid-cols-[minmax(0,1fr)_20rem]">
            <div className="min-w-0 space-y-4">
              {data.servers.length === 0 ? (
                <Panel className="flex flex-col items-start gap-3 p-8 sm:p-10">
                  <Eyebrow>No custom tools yet</Eyebrow>
                  <p className="font-display text-3xl font-semibold tracking-tight sm:text-4xl"><KineticText text="Plug in a tool." /></p>
                  <p className="max-w-md text-sm text-mute">Connect an MCP server (GitHub, Notion, your own) or a webhook, and your AI workers can use it.</p>
                  <Button className="mt-2" onClick={() => setAdding(true)}><Icon name="plus" size={16} /> Add your first tool</Button>
                </Panel>
              ) : (
                data.servers.map((s, i) => <ServerCard key={s.id} i={i} server={s} tools={data.tools.filter((t) => t.server_id === s.id)} onChange={reload} />)
              )}
            </div>
            <Standard tools={standard} onConnect={(t) => setAdding({ name: t.name, description: t.description })} />
          </div>
        </div>
      )}
      {adding && <AddTool preset={adding === true ? null : adding} onClose={() => setAdding(false)} onDone={() => { setAdding(false); reload(); }} />}
    </>
  );
}

// The standard tool names your roles and tasks use. Each is filled by an
// MCP server (or webhook) with the same name; tasks that need an unfilled
// one wait until it's connected.
function Standard({ tools, onConnect }) {
  return (
    <Panel i={1} className="p-6">
      <PanelHead eyebrow="Standard" title="Tools your roles use" />
      <p className="-mt-2 mb-4 text-sm text-mute">Connect an MCP server with the same name. Tasks that need one that isn't connected wait for it.</p>
      <ul className="space-y-2">
        {tools.map((t) => (
          <li key={t.name} className="flex items-center gap-3 rounded-xl bg-white/3 px-3 py-2.5 ring-1 ring-white/5">
            <span className={`size-1.5 shrink-0 rounded-full ${t.connected ? "bg-lime" : "bg-white/25"}`} aria-hidden="true" />
            <div className="min-w-0 flex-1">
              <p className="font-mono text-[13px]">{t.name}</p>
              <p className="truncate text-xs text-dim" title={t.description}>{t.description}</p>
            </div>
            {t.connected ? (
              <Badge tone="lime">Connected</Badge>
            ) : (
              <Button size="sm" variant="ghost" className="shrink-0" onClick={() => onConnect(t)} aria-label={`Connect ${t.name}`}>Connect</Button>
            )}
          </li>
        ))}
      </ul>
    </Panel>
  );
}

// An on/off switch in the control-room style.
function Toggle({ on, onChange, label: name, disabled, small = false }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={name}
      title={on ? "Turn off" : "Turn on"}
      disabled={disabled}
      onClick={() => onChange(!on)}
      className={`relative inline-flex shrink-0 items-center rounded-full p-0.5 transition-colors duration-300 disabled:opacity-40 ${small ? "h-5 w-9" : "h-6 w-11"} ${on ? "bg-lime" : "bg-white/10 ring-1 ring-inset ring-white/10 hover:bg-white/15"}`}
    >
      <span className={`block rounded-full shadow transition-transform duration-300 ${small ? "size-4" : "size-5"} ${on ? `bg-ink ${small ? "translate-x-4" : "translate-x-5"}` : "bg-fog/70"}`} />
    </button>
  );
}

function ServerCard({ server: s, tools, onChange, i }) {
  const call = useCompanyApi();
  const { notify } = useApp();
  const [busy, setBusy] = useState(false);
  const [removing, setRemoving] = useState(false);
  const mcp = s.kind === "mcp";
  const act = async (fn, ok) => {
    setBusy(true);
    try {
      const r = await fn();
      if (ok) notify(typeof ok === "function" ? ok(r) : ok);
    } catch (e) {
      notify(e.message, "error");
    }
    setBusy(false);
    onChange();
  };
  const remove = () => {
    setRemoving(false);
    act(() => call(`/admin/tools/${s.id}`, { method: "DELETE" }), `${s.name} removed.`);
  };
  const on = tools.filter((t) => t.enabled).length;
  // The dialog sits outside the panel: a panel is its own stacking context.
  return (
    <>
      <Panel i={i} className={`p-5 sm:p-6 ${s.enabled ? "" : "opacity-70"}`}>
        <div className="flex items-start gap-3">
          <span className={`flex size-10 shrink-0 items-center justify-center rounded-2xl ring-1 ring-inset ${s.enabled ? "bg-violet/15 text-violet ring-violet/30" : "bg-white/5 text-dim ring-white/10"}`}>
            <Icon name={mcp ? "tools" : "bolt"} />
          </span>
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="font-display text-lg font-medium tracking-tight">{s.name}</h2>
              <Badge>{mcp ? "MCP server" : "Webhook"}</Badge>
              {s.last_error ? <Badge tone="ember">Error</Badge> : s.enabled ? <Badge tone="lime" dot>On</Badge> : <Badge>Off</Badge>}
            </div>
            <p className="mt-0.5 truncate font-mono text-xs text-mute" title={s.url}>{s.url}</p>
          </div>
          <Toggle on={s.enabled} disabled={busy} label={`${s.name} server`} onChange={(v) => act(() => call(`/admin/tools/${s.id}/enabled`, { method: "POST", body: { enabled: v } }))} />
        </div>
  
        {s.description && <p className="mt-3 text-sm text-fog/90">{s.description}</p>}
        {s.last_error && <div className="mt-3"><ErrorNote error={s.last_error} /></div>}
  
        {tools.length > 0 && (
          <div className="mt-4 rounded-2xl bg-black/20 ring-1 ring-white/5">
            <p className="flex items-center justify-between px-4 pt-3 font-mono text-[10px] uppercase tracking-widest text-dim">
              <span>Tools</span>
              <span className="num">{on}/{tools.length} on</span>
            </p>
            <ul className="divide-y divide-white/5 px-4">
              {tools.map((t) => (
                <li key={t.id} className="flex items-center gap-3 py-3">
                  <div className="min-w-0 flex-1">
                    <p className={`truncate font-mono text-[13px] ${t.enabled ? "text-fog" : "text-dim"}`} title={t.tool_id}>{t.tool_id}</p>
                    {t.description && <p className="mt-0.5 line-clamp-2 text-xs text-mute">{t.description}</p>}
                  </div>
                  <Toggle small on={t.enabled} disabled={busy} label={`${t.tool_id} tool`} onChange={(v) => act(() => call(`/admin/tools/items/${t.id}/enabled`, { method: "POST", body: { enabled: v } }))} />
                </li>
              ))}
            </ul>
          </div>
        )}
        {mcp && tools.length === 0 && !s.last_error && <p className="mt-4 text-sm text-mute">No tools found on this server.</p>}
  
        <div className="mt-4 flex flex-wrap items-center gap-2 border-t border-white/5 pt-4">
          {s.secret_hint && <Chip>{s.auth_header}: {s.secret_hint}</Chip>}
          {s.last_synced_at && <span className="font-mono text-[11px] text-dim" title={when(s.last_synced_at)}>Synced {ago(s.last_synced_at)}</span>}
          <span className="ml-auto flex gap-2">
            {mcp && (
              <Button size="sm" variant="ghost" disabled={busy} onClick={() => act(() => call(`/admin/tools/${s.id}/refresh`, { method: "POST" }), (r) => `Tools refreshed: ${r?.tools ?? 0} found.`)} title="Refresh tools from the server">
                <Glyph d={REFRESH} /> Refresh
              </Button>
            )}
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => setRemoving(true)} className="hover:text-ember">
              <Glyph d={TRASH} /> Remove
            </Button>
          </span>
        </div>
      </Panel>

      <Modal open={removing} onClose={() => setRemoving(false)} kicker="Are you sure?" title={`Remove ${s.name}?`}>
        <p className="text-sm text-mute">
          Its tools are taken off every worker{tools.length ? ` (${tools.length} tool${tools.length === 1 ? "" : "s"})` : ""}. You can add it again later, but you'll have to give the tools back to each worker.
        </p>
        <div className="mt-6 grid grid-cols-2 gap-3">
          <Button variant="ghost" onClick={() => setRemoving(false)}>Cancel</Button>
          <Button variant="danger" disabled={busy} onClick={remove}><Glyph d={TRASH} size={16} /> Remove</Button>
        </div>
      </Modal>
    </>
  );
}

function AddTool({ preset, onClose, onDone }) {
  const call = useCompanyApi();
  const { notify } = useApp();
  const [form, setForm] = useState({ kind: "mcp", name: preset?.name || "", url: "", description: preset?.description || "", auth_header: "Authorization", auth_value: "" });
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(false);
  const set = (k) => (v) => setForm((f) => ({ ...f, [k]: v }));
  const mcp = form.kind === "mcp";
  const save = async (e) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await call("/admin/tools", { method: "POST", body: { ...form, name: form.name.trim().toLowerCase(), auth_value: form.auth_value || null } });
      notify(mcp ? `${form.name} added. Its tools are listed below.` : `${form.name} added.`);
      onDone();
    } catch (err) {
      setError(err.message);
    }
    setBusy(false);
  };
  return (
    <Modal open onClose={onClose} kicker={preset ? "Standard tool" : "Custom tool"} title={preset ? `Connect ${preset.name}` : "Add a tool"}>
      <form onSubmit={save} className="space-y-4">
        <Segmented
          label="Kind"
          value={form.kind}
          onChange={set("kind")}
          options={[
            { value: "mcp", label: "MCP server" },
            { value: "webhook", label: "Webhook" },
          ]}
        />
        <p className="text-sm text-mute">
          {mcp
            ? "An MCP server over HTTP (streamable HTTP). ForeMan connects, lists its tools, and adds each one as server-name.tool-name."
            : "ForeMan POSTs the task as JSON to this URL and stores the response. The tool is named after the name you choose."}
        </p>
        <Input label="Name" placeholder="github" value={form.name} onChange={set("name")} hint={preset ? `Keep it ${preset.name} so workers with ${preset.name} on their list use this server.` : "Lowercase letters, digits, - and _. Tools are called by this name."} className="[&_input]:font-mono" autoFocus={!preset} />
        <Input label="URL" placeholder={mcp ? "https://mcp.example.com/mcp" : "https://hooks.example.com/axom"} value={form.url} onChange={set("url")} autoFocus={!!preset} hint="Must be https and on the public internet." inputMode="url" className="[&_input]:font-mono" />
        <Input label="What it's for (optional)" value={form.description} onChange={set("description")} />
        <p className="flex items-center gap-3 pt-2 font-mono text-[10px] uppercase tracking-widest text-dim">
          <span className="h-px flex-1 bg-white/10" />
          Authentication (optional)
          <span className="h-px flex-1 bg-white/10" />
        </p>
        <div className="grid gap-4 sm:grid-cols-[11rem_minmax(0,1fr)]">
          <Input label="Header" value={form.auth_header} onChange={set("auth_header")} className="[&_input]:font-mono" />
          <Input label="Value" type="password" placeholder="Bearer sk-..." value={form.auth_value} onChange={set("auth_value")} autoComplete="off" hint="Encrypted; only the last four characters are shown again." />
        </div>
        <ErrorNote error={error} />
        <div className="grid grid-cols-2 gap-3 pt-2">
          <Button type="button" variant="ghost" onClick={onClose}>Cancel</Button>
          <Button type="submit" disabled={busy || form.name.trim().length < 2 || !form.url.trim()}>{busy ? (mcp ? "Connecting…" : "Saving…") : "Add tool"}</Button>
        </div>
      </form>
    </Modal>
  );
}
