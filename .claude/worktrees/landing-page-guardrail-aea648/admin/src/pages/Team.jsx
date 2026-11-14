// The org chart: departments, teams, and workers (AI agents and people), as
// cards or as a live network around the policy engine.
import { useState } from "react";
import OrgNetwork from "../components/OrgNetwork.jsx";
import {
  Addr, Avatar, Badge, Button, Chip, CopyField, Empty, ErrorNote, Eyebrow, Icon, Loading, Modal, PageHead, Panel, Segmented, StatusBadge,
  human, useApp, useCompanyApi, useLoad, useSiteUrl, usd,
} from "../ui.jsx";

const VIEW_KEY = "axom-team-view";
function readView() {
  try {
    const v = localStorage.getItem(VIEW_KEY);
    return v === "cards" || v === "network" ? v : null;
  } catch {
    return null;
  }
}
const wideScreen = () => typeof window !== "undefined" && !!window.matchMedia?.("(min-width: 900px)").matches;

export default function Team() {
  const { data, error, loading, reload } = useLoad("/org-chart");
  const call = useCompanyApi();
  const { notify, navigate } = useApp();
  const [view, setView] = useState(() => readView() || (wideScreen() ? "network" : "cards"));
  const [filter, setFilter] = useState("all");
  const [selected, setSelected] = useState(null);
  const [editing, setEditing] = useState(null);
  const [token, setToken] = useState(null);
  const [issuing, setIssuing] = useState(null);
  const noPlan = error && /no plan/i.test(error);
  const depts = data?.departments || [];
  const all = depts.flatMap((d) => d.teams.flatMap((t) => t.workers));
  const ai = all.filter((w) => w.kind === "ai_agent").length;
  const show = (w) => filter === "all" || (filter === "ai" ? w.kind === "ai_agent" : w.kind !== "ai_agent");
  // The selected worker, fresh from the latest load (tools may have changed).
  const current = selected && all.find((w) => w.id === selected.id);

  const pickView = (v) => {
    setView(v);
    try {
      localStorage.setItem(VIEW_KEY, v);
    } catch {
      /* ignore */
    }
  };
  // One request at a time: a double click would mint two tokens.
  const issue = async (w) => {
    if (issuing) return;
    setIssuing(w.id);
    try {
      const r = await call(`/workers/${w.id}/intake-token`, { method: "POST" });
      setToken({ worker: w, token: r.token });
    } catch (e) {
      notify(e.message, "error");
    }
    setIssuing(null);
  };
  const card = (w, i) => <WorkerCard key={w.id} w={w} i={i} onTools={setEditing} onIssue={issue} issuing={issuing} />;

  return (
    <>
      <PageHead kicker="Built from the master plan" title="Org chart">
        {!noPlan && (
          <div className="flex flex-wrap items-end gap-3">
            {view === "cards" && data && (
              <Segmented
                value={filter}
                onChange={setFilter}
                options={[
                  { value: "all", label: `All ${all.length}` },
                  { value: "ai", label: `AI ${ai}` },
                  { value: "human", label: `People ${all.length - ai}` },
                ]}
              />
            )}
            <Segmented value={view} onChange={pickView} options={[{ value: "network", label: "Network" }, { value: "cards", label: "Cards" }]} />
          </div>
        )}
      </PageHead>
      <p className="reveal -mt-2 mb-6 max-w-2xl text-sm text-mute">Departments, teams, and the AI agents and people in them.</p>

      {noPlan ? (
        <Panel className="flex flex-col items-start gap-3 p-8 sm:p-12">
          <Eyebrow className="text-amber opacity-100">No plan yet</Eyebrow>
          <p className="max-w-xl font-display text-2xl font-semibold tracking-tight sm:text-4xl">Set your company's goal in Settings to build the team.</p>
          <p className="max-w-md text-sm text-mute">The departments, teams and workers on this page all come from the plan.</p>
          <Button className="mt-3" onClick={() => navigate("settings")}>
            Open Settings <Icon name="arrow" size={16} />
          </Button>
        </Panel>
      ) : (
        <>
          <ErrorNote error={error} />
          {loading && <Loading rows={6} />}
          {data && depts.length === 0 && <Empty title="No departments yet" text="They appear here once the plan is built." />}
        </>
      )}

      {!noPlan && view === "network" && depts.length > 0 && (
        <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_20rem]">
          <Panel className="p-0">
            <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(circle_at_center,rgba(139,108,255,.12),transparent_60%)]" />
            <OrgNetwork departments={depts} selected={current?.id} onSelect={setSelected} />
          </Panel>
          <div className="space-y-4">
            {current ? (
              <>
                {card(current, 0)}
                <Button variant="ghost" size="sm" className="w-full" onClick={() => setSelected(null)}>Clear selection</Button>
              </>
            ) : (
              <Panel i={1} className="p-6">
                <Eyebrow>Live routing</Eyebrow>
                <p className="mt-3 font-display text-xl/tight font-semibold">Every task leaves the policy engine and travels to a worker.</p>
                <p className="mt-3 text-sm text-mute">Hover a worker to trace its route. Click to open its card; click a department to zoom in.</p>
                <dl className="mt-6 grid grid-cols-2 gap-2 text-sm">
                  <Stat label="Departments" value={depts.length} />
                  <Stat label="Teams" value={depts.reduce((n, d) => n + d.teams.length, 0)} />
                  <Stat label="AI agents" value={ai} tone="violet" />
                  <Stat label="People" value={all.length - ai} tone="sky" />
                </dl>
              </Panel>
            )}
          </div>
        </div>
      )}

      {!noPlan && view === "cards" && depts.length > 0 && (
        <div className="space-y-10">
          {depts.map((d, di) => {
            const count = d.teams.reduce((n, t) => n + t.workers.length, 0);
            return (
              <section key={d.id} className="reveal" style={{ "--i": di }} aria-label={d.name}>
                <header className="mb-4 flex flex-wrap items-baseline gap-x-4 gap-y-1 border-b border-white/10 pb-3">
                  <span className="font-mono text-xs text-lime">{String(di + 1).padStart(2, "0")}</span>
                  <h2 className="font-display text-2xl font-semibold tracking-tight sm:text-3xl">{d.name}</h2>
                  <span className="ml-auto text-xs text-mute">
                    {d.teams.length} team{d.teams.length === 1 ? "" : "s"} · {count} worker{count === 1 ? "" : "s"}
                  </span>
                </header>
                <div className="grid gap-4">
                  {d.teams.map((t) => {
                    const shown = t.workers.filter(show);
                    return (
                      <div key={t.id} className="rounded-3xl border border-white/5 bg-white/1.5 p-3">
                        <h3 className="mb-3 px-2 pt-1 font-mono text-[11px] uppercase tracking-[0.18em] text-mute">{t.name}</h3>
                        {t.workers.length === 0 ? (
                          <p className="flex items-center gap-2 px-2 pb-2 text-xs text-amber">
                            <span className="pulse-dot rounded-full bg-amber size-1.5" />
                            No one yet: a hiring request is open for this team.
                          </p>
                        ) : shown.length === 0 ? (
                          <p className="px-2 pb-2 text-xs text-dim">No matching workers.</p>
                        ) : (
                          <div className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,16rem),1fr))] gap-3">{shown.map(card)}</div>
                        )}
                      </div>
                    );
                  })}
                </div>
              </section>
            );
          })}
        </div>
      )}

      {editing && (
        <ToolsDialog
          worker={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            reload();
          }}
        />
      )}
      <BriefTokenDialog token={token} onClose={() => setToken(null)} />
    </>
  );
}

function Stat({ label, value, tone }) {
  const ring = tone === "violet" ? "bg-violet/10 ring-violet/20" : tone === "sky" ? "bg-sky/10 ring-sky/20" : "bg-white/3 ring-white/5";
  const text = tone === "violet" ? "text-violet" : tone === "sky" ? "text-sky" : "";
  return (
    <div className={`rounded-xl p-3 ring-1 ${ring}`}>
      <dt className="text-xs text-mute">{label}</dt>
      <dd className={`num font-display text-2xl ${text}`}>{value}</dd>
    </div>
  );
}

function WorkerCard({ w, i = 0, onTools, onIssue, issuing }) {
  const ai = w.kind === "ai_agent";
  const fired = w.status === "fired";
  const tools = w.tool_allowlist || [];
  return (
    <Panel i={i} className={`p-4 ${fired ? "opacity-60" : ""}`}>
      <div className="flex items-start gap-3">
        <Avatar name={w.name} kind={w.kind} size={44} />
        <div className="min-w-0 flex-1">
          <p className={`line-clamp-2 font-medium leading-snug ${fired ? "line-through" : ""}`} title={w.name}>{w.name}</p>
          <p className="truncate text-xs text-mute" title={w.role}>{w.role}</p>
        </div>
        <StatusBadge status={w.status} dot={w.status === "active"} />
      </div>
      <div className="mt-4 flex items-baseline gap-1">
        <span className="num font-display text-2xl font-semibold">{usd(w.pay_rate_usd, Number(w.pay_rate_usd) % 1 ? 2 : 0)}</span>
        <span className="text-xs text-mute">per {human(w.pay_unit)}</span>
        <Badge tone={ai ? "violet" : "sky"} className="ml-auto">{ai ? "AI agent" : "Freelancer"}</Badge>
      </div>
      {w.skills?.length > 0 && (
        <div className="mt-3 flex flex-wrap gap-1">{w.skills.map((s) => <Chip key={s}>{human(s)}</Chip>)}</div>
      )}
      {w.kpis?.length > 0 && (
        <ul className="mt-3 space-y-1 border-t border-white/5 pt-3 text-xs text-mute" aria-label="KPIs">
          {w.kpis.map((k) => (
            <li key={k.name} className="flex justify-between gap-2">
              <span>{human(k.name)}</span>
              <span className="num text-fog">{k.higher_is_better ? "≥" : "≤"} {k.target}</span>
            </li>
          ))}
        </ul>
      )}
      {ai && (
        <div className="mt-3 border-t border-white/5 pt-3">
          <div className="mb-2 flex items-center gap-2">
            <span className="mr-auto font-mono text-[10px] uppercase tracking-widest text-dim">Tools</span>
            <Button size="sm" variant="ghost" onClick={() => onTools(w)} aria-label={`${tools.length ? "Edit" : "Give"} tools for ${w.name}`}>
              <Icon name="tools" size={13} /> {tools.length ? "Edit tools" : "Give tools"}
            </Button>
          </div>
          {tools.length > 0 ? (
            <div className="flex flex-wrap gap-1">{tools.map((t) => <ToolChip key={t} name={t} />)}</div>
          ) : (
            <p className="text-xs text-dim">No tools yet, so it can't run tool tasks.</p>
          )}
        </div>
      )}
      {!ai && w.status === "active" && (
        <Button size="sm" variant="ghost" className="mt-4 w-full" disabled={!!issuing} onClick={() => onIssue(w)}>
          <Icon name="key" size={14} /> {issuing === w.id ? "Issuing…" : "Issue brief token"}
        </Button>
      )}
      {w.wallet_address && (
        <p className="mt-3 flex min-w-0 items-center gap-1 text-[11px] text-dim">
          <span aria-hidden="true">◎</span> <Addr value={w.wallet_address} />
        </p>
      )}
    </Panel>
  );
}

function ToolChip({ name }) {
  return (
    <span className="inline-flex items-center gap-1 rounded-md bg-violet/10 px-2 py-0.5 font-mono text-[11px] text-violet ring-1 ring-inset ring-violet/20">
      <Icon name="tools" size={11} />
      {name}
    </span>
  );
}

// A freelancer's key to the briefs portal. Shown once; issuing again makes a new one.
function BriefTokenDialog({ token, onClose }) {
  const site = useSiteUrl();
  return (
    <Modal open={!!token} onClose={onClose} backdropClose={false} kicker="Shown once" title={`Brief token for ${token?.worker.name}`}>
      <p className="mb-4 text-sm text-mute">
        Send this to the freelancer over a private channel. They paste it into the freelancer portal to see their briefs and submit deliverables.
        It's shown only once; issuing a new token doesn't show this one again.
      </p>
      {token && (
        <div className="space-y-2">
          <CopyField label="Brief token" value={token.token} />
          <CopyField label="Freelancer portal" value={site("/briefs")} />
        </div>
      )}
      <Button className="mt-6 w-full" onClick={onClose}>Done</Button>
    </Modal>
  );
}

function ToolsDialog({ worker, onClose, onSaved }) {
  const call = useCompanyApi();
  const { notify, navigate } = useApp();
  const tools = useLoad("/admin/tools");
  const [picked, setPicked] = useState(() => new Set(worker.tool_allowlist));
  const [busy, setBusy] = useState(false);
  const toggle = (t) => {
    const next = new Set(picked);
    if (next.has(t)) next.delete(t);
    else next.add(t);
    setPicked(next);
  };
  const save = async () => {
    setBusy(true);
    try {
      await call(`/admin/workers/${worker.id}/tools`, { method: "POST", body: { tools: [...picked] } });
      notify(`${worker.name}'s tools updated. The change is in the audit log.`);
      onSaved();
    } catch (e) {
      notify(e.message, "error");
    }
    setBusy(false);
  };
  const builtin = tools.data?.builtin || [];
  const custom = (tools.data?.available || []).filter((t) => !builtin.includes(t));
  const group = (title, list) =>
    list.length > 0 && (
      <fieldset className="mt-5">
        <legend className="mb-2 font-mono text-[10px] uppercase tracking-widest text-dim">{title}</legend>
        <div className="grid gap-2 sm:grid-cols-2">
          {list.map((t) => (
            <label
              key={t}
              className={`flex cursor-pointer items-center gap-3 rounded-xl px-3 py-2.5 ring-1 ring-inset transition ${picked.has(t) ? "bg-violet/10 ring-violet/30" : "bg-white/3 ring-white/10 hover:bg-white/5"}`}
            >
              <input type="checkbox" checked={picked.has(t)} onChange={() => toggle(t)} className="accent-lime size-4 shrink-0" />
              <span className="min-w-0 truncate font-mono text-[13px]">{t}</span>
            </label>
          ))}
        </div>
      </fieldset>
    );
  return (
    <Modal open onClose={onClose} kicker="Tool allowlist" title={`Tools for ${worker.name}`}>
      <p className="flex gap-2 rounded-xl bg-sky/10 px-3 py-2.5 text-sm text-sky ring-1 ring-inset ring-sky/25">
        <Icon name="platform" size={16} className="mt-0.5 shrink-0" />
        The worker can only use tools on this list, and every use is still checked and logged.
      </p>
      <div className="mt-4">
        <ErrorNote error={tools.error} />
        {tools.loading && <Loading rows={4} />}
        {tools.data && (
          <>
            {group("Your tools", custom)}
            {group("Built-in", builtin)}
            {custom.length + builtin.length === 0 && (
              <p className="mt-4 text-sm text-mute">
                No tools are available yet.{" "}
                <Button variant="link" size="sm" onClick={() => navigate("tools")}>Add one on the Tools page</Button>
              </p>
            )}
          </>
        )}
      </div>
      <div className="mt-6 grid grid-cols-2 gap-3">
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
        <Button onClick={save} disabled={busy}>{busy ? "Saving…" : "Save"}</Button>
      </div>
    </Modal>
  );
}
