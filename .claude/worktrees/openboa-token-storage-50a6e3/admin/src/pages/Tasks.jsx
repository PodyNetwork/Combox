// Every task, its status and requirements; and adding a new one. Combox routes
// new tasks to a capable worker, or opens a hiring request.
import { useMemo, useState } from "react";
import DateTimePicker from "../DateTimePicker.jsx";
import { ReplyBox } from "./AgentLog.jsx";
import {
  Avatar, Button, Chip, Empty, ErrorNote, Icon, Input, Loading, Modal, PageHead, Panel, Segmented, Textarea,
  human, localDate, useApp, useCompanyApi, useLoad, useToast, usd,
} from "../ui.jsx";

const COLUMNS = [
  { key: "queued", label: "Queued", dot: "bg-white/40" },
  { key: "assigned", label: "Assigned", dot: "bg-sky" },
  { key: "in_progress", label: "In progress", dot: "bg-violet" },
  { key: "blocked", label: "Blocked", dot: "bg-amber" },
  { key: "done", label: "Done", dot: "bg-lime" },
  { key: "failed", label: "Failed", dot: "bg-ember" },
];
const FILTERS = { open: ["queued", "assigned", "in_progress", "blocked"], done: ["done"], failed: ["failed"], all: null };
const PRIO = { 1: "bg-ember text-ink", 2: "bg-amber text-ink", 3: "bg-white/10 text-fog", 4: "bg-white/5 text-mute", 5: "bg-white/5 text-dim" };
const EMPTY = { objective: "", skills: "", tools: [], priority: 3, deadline: "" };
// "15:30", or "Nov 15, 15:30" when it isn't today.
const timeOf = (iso) => {
  const d = new Date(iso);
  const time = d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  return d.toDateString() === new Date().toDateString() ? time : `${d.toLocaleDateString(undefined, { month: "short", day: "numeric" })}, ${time}`;
};
// Deliverable URLs are untrusted; only ever link http(s).
const safeHref = (u) => (/^https?:\/\//i.test(u || "") ? u : undefined);
const skillList = (s) => s.split(",").map((x) => x.trim().toLowerCase().replace(/\s+/g, "_")).filter(Boolean);

export default function Tasks() {
  // Polled, so automatic retries and reruns show up without a reload.
  const { data, error, loading, reload } = useLoad("/tasks", { poll: 15000 });
  const call = useCompanyApi();
  const toast = useToast();
  const [retrying, setRetrying] = useState(null);
  const [answering, setAnswering] = useState(null);
  const org = useLoad("/org-chart");
  const tools = useLoad("/admin/tools");
  const [filter, setFilter] = useState("open");
  const [creating, setCreating] = useState(false);
  const [reviewing, setReviewing] = useState(null);
  const [viewing, setViewing] = useState(null);

  const workers = useMemo(() => {
    const m = {};
    (org.data?.departments || []).forEach((d) => d.teams.forEach((t) => t.workers.forEach((w) => (m[w.id] = w))));
    return m;
  }, [org.data]);

  const all = data?.tasks || [];
  const spend = all.reduce((s, t) => s + Number(t.cost_usd || 0), 0);
  const count = (f) => (FILTERS[f] ? all.filter((t) => FILTERS[f].includes(t.status)).length : all.length);
  const columns = COLUMNS.filter((c) => !FILTERS[filter] || FILTERS[filter].includes(c.key));
  const single = columns.length === 1;

  // Send a failed task back to the queue; a worker picks it up within a minute.
  const retry = async (t) => {
    setRetrying(t.id);
    try {
      await call(`/admin/tasks/${t.id}/retry`, { method: "POST" });
      toast("Queued again. A worker picks it up within a minute.");
      reload();
    } catch (e) {
      toast(e.message, "error");
    } finally {
      setRetrying(null);
    }
  };

  return (
    <>
      <PageHead kicker={data ? `${all.length} task${all.length === 1 ? "" : "s"} · ${usd(spend)} spent` : "Task intake"} title="Task board">
        <Button onClick={() => setCreating(true)}>
          <Icon name="plus" size={16} /> New task
        </Button>
      </PageHead>
      <p className="reveal -mt-2 mb-6 max-w-2xl text-sm text-mute">Combox routes each task to a capable worker, or opens a hiring request.</p>
      <div className="reveal mb-5" style={{ "--i": 1 }}>
        <Segmented
          value={filter}
          onChange={setFilter}
          options={[
            { value: "open", label: `Open ${data ? count("open") : ""}` },
            { value: "done", label: `Done ${data ? count("done") : ""}` },
            { value: "failed", label: `Failed ${data ? count("failed") : ""}` },
            { value: "all", label: `All ${data ? count("all") : ""}` },
          ]}
        />
      </div>
      <ErrorNote error={error} />
      {loading && <Loading rows={5} />}
      {data && count(filter) === 0 && <Empty title="No tasks here." text="Add one with New task. Combox finds a worker for it or opens a hiring request." />}
      {data && count(filter) > 0 && (
        <div className={single ? "" : "scrollbar-none -mx-4 flex snap-x scroll-px-4 gap-4 overflow-x-auto px-4 pb-4 sm:-mx-8 sm:scroll-px-8 sm:px-8"}>
          {columns.map((c, ci) => {
            const items = all.filter((t) => t.status === c.key).sort((a, b) => a.priority - b.priority);
            return (
              <section key={c.key} className={`reveal ${single ? "" : "w-76 shrink-0 snap-start lg:w-auto lg:min-w-64 lg:flex-1 lg:basis-0"}`} style={{ "--i": ci }} aria-label={c.label}>
                <header className="mb-3 flex items-center gap-2 px-1">
                  <span className={`rounded-full size-2 ${c.dot}`} />
                  <h2 className="font-display text-sm font-medium">{c.label}</h2>
                  <span className="ml-auto rounded-full bg-white/5 px-2 py-0.5 font-mono text-[11px] text-mute">{items.length}</span>
                </header>
                <div className={`min-h-32 rounded-3xl bg-white/1.5 p-2 ring-1 ring-white/5 ${single ? "grid content-start gap-3 sm:grid-cols-2 xl:grid-cols-3" : "space-y-3"}`}>
                  {items.map((t, i) => (
                    <TaskCard key={t.id} t={t} i={i} worker={workers[t.assignee]} retrying={retrying === t.id} onRetry={() => retry(t)} onAnswer={() => setAnswering({ task: t, worker: workers[t.assignee] })} onReview={() => setReviewing({ task: t, worker: workers[t.assignee] })} onView={() => setViewing({ task: t, worker: workers[t.assignee] })} />
                  ))}
                  {items.length === 0 && <p className="px-3 py-6 text-center text-xs text-dim">Empty</p>}
                </div>
              </section>
            );
          })}
        </div>
      )}
      <NewTask open={creating} onClose={() => setCreating(false)} tools={tools} onDone={reload} />
      <Answer data={answering} onClose={() => setAnswering(null)} onSent={() => { setAnswering(null); toast("Reply sent. The worker carries on within a minute."); reload(); }} />
      <WorkReport task={viewing?.task} worker={viewing?.worker} onClose={() => setViewing(null)} />
      <Review key={`review-${reviewing?.task.id ?? "none"}`} data={reviewing} onClose={() => setReviewing(null)} onDone={reload} />
    </>
  );
}

function TaskCard({ t, i, worker, retrying, onRetry, onAnswer, onReview, onView }) {
  const asking = t.status === "blocked" && t.result?.waiting_for === "reply";
  const reviewable = worker?.kind === "human_freelancer" && t.status === "in_progress";
  const hasReport = worker?.kind === "ai_agent" && (t.result?.summary || t.result?.error || t.result?.tool_calls?.length || t.result?.tool_outputs?.length);
  const deliverable = t.result?.deliverable;
  const skills = t.required_capabilities?.skills || [];
  const tools = t.required_capabilities?.tools || [];
  const due = t.deadline && localDate(t.deadline);
  const late = due && !["done", "failed"].includes(t.status) && due < new Date(new Date().toDateString());
  return (
    <Panel i={i} className="rounded-2xl! p-4">
      <div className="mb-2 flex items-center gap-2">
        <span className={`rounded-md px-1.5 py-0.5 font-mono text-[10px] font-semibold ${PRIO[t.priority] || PRIO[3]}`} title={`Priority ${t.priority} of 5 (1 is highest)`}>P{t.priority}</span>
        {due && (
          <span className={`font-mono text-[11px] ${late ? "text-ember" : "text-mute"}`}>
            {late ? "overdue " : "due "}
            {due.toLocaleDateString(undefined, { month: "short", day: "numeric", year: due.getFullYear() === new Date().getFullYear() ? undefined : "numeric" })}
          </span>
        )}
        {Number(t.cost_usd) > 0 && <span className="num ml-auto text-[11px] text-mute">{usd(t.cost_usd)}</span>}
      </div>
      <p className="text-sm/snug">{t.objective}</p>
      {(skills.length > 0 || tools.length > 0) && (
        <div className="mt-3 flex flex-wrap gap-1" aria-label="Needs">
          {skills.map((s) => <Chip key={s}>{human(s)}</Chip>)}
          {tools.map((s) => (
            <span key={s} className="inline-flex items-center gap-1 rounded-md bg-violet/10 px-2 py-0.5 font-mono text-[11px] text-violet ring-1 ring-inset ring-violet/20" title="Tool: only an AI agent with it can do this task">
              <Icon name="tools" size={11} />
              {s}
            </span>
          ))}
        </div>
      )}
      {t.result?.summary && <p className="mt-3 line-clamp-3 rounded-lg bg-lime/5 px-2.5 py-1.5 text-xs text-mute ring-1 ring-lime/15">{t.result.summary}</p>}
      {asking && (
        <div className="mt-3 rounded-lg bg-amber/8 px-2.5 py-2 ring-1 ring-amber/25">
          <p className="mb-1 font-mono text-[10px] uppercase tracking-widest text-amber">Question for you</p>
          <p className="line-clamp-4 whitespace-pre-wrap text-xs text-fog/85">{t.result.questions}</p>
        </div>
      )}
      {t.result?.error && <p className="mt-3 line-clamp-2 rounded-lg bg-ember/5 px-2.5 py-1.5 text-xs text-ember ring-1 ring-ember/20">{t.result.error}</p>}
      {t.status === "failed" && (
        <p className="mt-2 flex items-center gap-1.5 text-[11px] text-mute">
          <Icon name="clock" size={12} className="shrink-0" />
          {t.auto_retry
            ? `Retrying automatically at ${timeOf(t.auto_retry.at)} (${t.auto_retry.attempt} of ${t.auto_retry.max})`
            : "Not retried automatically. Retry it when the cause is fixed."}
        </p>
      )}
      {deliverable?.url && (
        <a href={safeHref(deliverable.url)} target="_blank" rel="noreferrer noopener" className="mt-3 block truncate rounded-lg bg-violet/10 px-2.5 py-1.5 font-mono text-[11px] text-violet ring-1 ring-violet/20 hover:bg-violet/20" title="Untrusted link from the freelancer">
          ↗ {deliverable.url}
        </a>
      )}
      <div className="mt-3 flex items-center gap-2 border-t border-white/5 pt-3">
        {worker ? (
          <>
            <Avatar name={worker.name} kind={worker.kind} size={24} tag={false} />
            <span className="min-w-0 truncate text-xs text-mute">{worker.name}</span>
          </>
        ) : (
          <span className="text-xs text-dim">{t.assignee ? "Assigned" : "Unassigned"}</span>
        )}
        <div className="ml-auto flex shrink-0 gap-1.5">
          {reviewable && <Button size="sm" onClick={onReview}>Review</Button>}
          {asking && <Button size="sm" onClick={onAnswer}>Reply</Button>}
          {hasReport && <Button size="sm" variant="ghost" onClick={onView}>Work</Button>}
          {t.status === "failed" && (
            <Button size="sm" onClick={onRetry} disabled={retrying} title="Send this task back to the queue">
              {retrying ? "Retrying…" : "Retry"}
            </Button>
          )}
        </div>
      </div>
    </Panel>
  );
}

function NewTask({ open, onClose, tools, onDone }) {
  const call = useCompanyApi();
  const { notify } = useApp();
  const [form, setForm] = useState(EMPTY);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(null);
  const set = (k) => (v) => setForm((f) => ({ ...f, [k]: v }));
  const options = tools.data?.available || [];
  const skills = skillList(form.skills);
  const toggleTool = (t) => set("tools")(form.tools.includes(t) ? form.tools.filter((x) => x !== t) : [...form.tools, t]);
  const close = () => {
    setError(null);
    onClose();
  };
  const submit = async (e) => {
    e.preventDefault();
    if (!form.objective.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const r = await call("/intake/tasks", {
        method: "POST",
        body: {
          objective: form.objective,
          priority: Number(form.priority),
          required_skills: skills,
          required_tools: form.tools,
          deadline: form.deadline || null,
        },
      });
      notify(`Task added (queue position ${r.queue_position}).`);
      setForm(EMPTY);
      onDone();
      onClose();
    } catch (err) {
      setError(err.message);
    }
    setBusy(false);
  };
  return (
    <Modal open={open} onClose={close} backdropClose={false} kicker="Task intake" title="New task">
      <form onSubmit={submit} className="space-y-5">
        <Textarea label="What needs doing?" value={form.objective} onChange={set("objective")} rows={3} placeholder="What needs to be true when this is done?" maxLength={2000} required autoFocus />
        <div>
          <Input label="Skills needed (comma-separated)" value={form.skills} onChange={set("skills")} placeholder="copywriting, visual design" />
          {skills.length > 0 && <div className="mt-2 flex flex-wrap gap-1">{skills.map((s) => <Chip key={s}>{s}</Chip>)}</div>}
        </div>
        <fieldset>
          <legend className="text-sm text-mute">Tools needed <span className="text-dim">(an AI worker with these tools does it)</span></legend>
          <div className="mt-2">
            <ErrorNote error={tools.error} />
            {tools.loading && <p className="text-xs text-dim">Loading tools…</p>}
            {tools.data && options.length === 0 && <p className="text-xs text-dim">No tools are available yet.</p>}
            {options.length > 0 && (
              <div className="flex flex-wrap gap-1.5">
                {options.map((t) => {
                  const on = form.tools.includes(t);
                  return (
                    <button
                      key={t}
                      type="button"
                      aria-pressed={on}
                      onClick={() => toggleTool(t)}
                      className={`inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 font-mono text-xs ring-1 ring-inset transition ${on ? "bg-violet/20 text-violet ring-violet/40" : "bg-white/3 text-mute ring-white/10 hover:bg-white/6 hover:text-fog"}`}
                    >
                      <Icon name={on ? "check" : "plus"} size={12} />
                      {t}
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </fieldset>
        <div>
          <span className="block text-sm text-mute">Priority</span>
          <Segmented value={form.priority} onChange={set("priority")} options={[1, 2, 3, 4, 5].map((p) => ({ value: p, label: `P${p}` }))} />
          <span className="mt-1 block text-xs text-dim">P1 is the highest, P5 the lowest.</span>
        </div>
        <DateTimePicker label="Deadline" value={form.deadline} onChange={set("deadline")} time={false} placeholder="No deadline" />
        <ErrorNote error={error} />
        <div className="grid grid-cols-2 gap-3 pt-1">
          <Button type="button" variant="ghost" onClick={close}>Cancel</Button>
          <Button type="submit" disabled={busy || !form.objective.trim()}>{busy ? "Adding…" : "Add task"}</Button>
        </div>
      </form>
    </Modal>
  );
}

// Answer an AI worker's questions on a task.
function Answer({ data, onClose, onSent }) {
  if (!data) return null;
  const { task, worker } = data;
  return (
    <Modal open onClose={onClose} kicker={`Question from ${worker?.name || "the AI worker"}`} title={task.objective}>
      <div className="mb-2 rounded-2xl bg-amber/8 p-4 ring-1 ring-amber/25">
        <p className="whitespace-pre-wrap text-sm">{task.result?.questions}</p>
      </div>
      <div className="-mx-5 -mb-4">
        <ReplyBox taskId={task.id} worker={worker?.name || "the worker"} onSent={onSent} autoFocus />
      </div>
    </Modal>
  );
}

// What an AI worker did: its write-up, each tool call (output is untrusted
// data from outside systems), and the Claude usage.
function WorkReport({ task, worker, onClose }) {
  if (!task) return null;
  const r = task.result || {};
  const calls = r.tool_calls || r.tool_outputs || [];
  return (
    <Modal open onClose={onClose} kicker={`Work · ${worker?.name || "AI worker"}`} title={task.objective}>
      {r.summary ? (
        <div className="mb-4 rounded-2xl bg-lime/5 p-4 ring-1 ring-lime/20">
          <p className="mb-1 font-mono text-[10px] uppercase tracking-widest text-lime">Report{r.model ? ` · ${r.model}` : ""}</p>
          <p className="whitespace-pre-wrap text-sm">{r.summary}</p>
        </div>
      ) : (
        !r.error && <p className="mb-4 text-sm text-mute">This worker ran its tools without Claude (no Anthropic key set), so there is no write-up.</p>
      )}
      {r.error && <p className="mb-4 rounded-2xl bg-ember/5 p-4 text-sm text-ember ring-1 ring-ember/20">{r.error}</p>}
      {calls.length > 0 && (
        <div className="space-y-2">
          <p className="font-mono text-[10px] uppercase tracking-widest text-dim">{calls.length} tool call{calls.length === 1 ? "" : "s"}</p>
          {calls.map((c, n) => (
            <details key={n} className="rounded-xl bg-white/3 px-3 py-2 text-sm ring-1 ring-white/10">
              <summary className="cursor-pointer font-mono text-xs text-violet">{c.tool || c.source}{c.error ? " · error" : ""}</summary>
              {c.arguments && <pre className="mt-2 overflow-x-auto whitespace-pre-wrap font-mono text-[11px] text-mute">{JSON.stringify(c.arguments, null, 2)}</pre>}
              {c.error && <p className="mt-2 text-xs text-ember">{c.error}</p>}
              {c.untrusted_content && (
                <>
                  <p className="mt-2 font-mono text-[10px] uppercase tracking-widest text-amber">Untrusted · tool output</p>
                  <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap font-mono text-[11px] text-mute">{c.untrusted_content}</pre>
                </>
              )}
            </details>
          ))}
        </div>
      )}
      {r.usage && (
        <p className="mt-4 text-center text-xs text-dim">
          <span className="num">{(r.usage.input_tokens || 0).toLocaleString()}</span> tokens in · <span className="num">{(r.usage.output_tokens || 0).toLocaleString()}</span> out
        </p>
      )}
    </Modal>
  );
}

// Accept or return a freelancer's deliverable. Accepting completes the task,
// which feeds payroll; the criteria check feeds performance reviews.
function Review({ data, onClose, onDone }) {
  const call = useCompanyApi();
  const { notify, refreshStatus } = useApp();
  const [criteria, setCriteria] = useState(true);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(null);
  if (!data) return null;
  const d = data.task.result?.deliverable;
  const act = async (accept) => {
    setBusy(true);
    setError(null);
    try {
      await call(`/intake/briefs/${data.task.id}/review`, { method: "POST", body: { accept, criteria_met: accept && criteria, note } });
      notify(accept ? "Accepted. It now counts toward payroll." : "Returned to the freelancer.");
      onDone();
      refreshStatus?.();
      onClose();
    } catch (e) {
      setError(e.message);
    }
    setBusy(false);
  };
  return (
    <Modal open onClose={onClose} kicker={`Deliverable · ${data.worker?.name || "freelancer"}`} title={data.task.objective}>
      {d?.url ? (
        <div className="mb-4 rounded-2xl bg-white/3 p-4 ring-1 ring-white/10">
          <p className="mb-1 font-mono text-[10px] uppercase tracking-widest text-amber">Untrusted · from the freelancer</p>
          <a href={safeHref(d.url)} target="_blank" rel="noreferrer noopener" className="block truncate font-mono text-sm text-violet hover:underline">{d.url}</a>
          {d.notes && <p className="mt-2 whitespace-pre-wrap text-sm text-mute">{d.notes}</p>}
        </div>
      ) : (
        <p className="mb-4 text-sm text-mute">Open the deliverable the freelancer submitted, then accept or return it.</p>
      )}
      <label className="mb-4 flex flex-wrap items-center gap-3 rounded-2xl bg-white/3 p-4 text-sm ring-1 ring-white/10">
        <input type="checkbox" checked={criteria} onChange={(e) => setCriteria(e.target.checked)} className="accent-lime size-4" />
        <span className="mr-auto">Meets the task's acceptance criteria</span>
        <span className="text-xs text-dim">feeds performance reviews</span>
      </label>
      <Textarea label="Note to the freelancer" value={note} onChange={setNote} rows={3} />
      <div className="mt-4">
        <ErrorNote error={error} />
      </div>
      <div className="mt-4 grid grid-cols-2 gap-3">
        <Button variant="ghost" disabled={busy} onClick={() => act(false)}>Return</Button>
        <Button disabled={busy} onClick={() => act(true)}>
          <Icon name="check" size={16} /> Accept
        </Button>
      </div>
      <p className="mt-3 text-center text-xs text-dim">Requires an MFA-verified session. Audited either way.</p>
    </Modal>
  );
}
