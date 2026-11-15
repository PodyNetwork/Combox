// The live agent log: what each AI worker is thinking and doing, one session
// per task. Claude's thinking summaries and replies stream in as they are
// written; tool calls and status changes appear as they happen. When a worker
// asks a question, the session waits for a reply, which you write here.
import { useEffect, useMemo, useRef, useState } from "react";
import { Avatar, Badge, Button, Empty, ErrorNote, Icon, Loading, PageHead, Panel, ago, tidy, useCompanyApi, when } from "../ui.jsx";

// How often to ask for changes. Only new or changed entries come back.
const POLL_MS = 1500;
// A session whose last status is "started" but that has been quiet this long
// is not shown as live (the server may have restarted mid-task).
const STALE_MS = 10 * 60 * 1000;

const STATE_TONE = { started: "violet", done: "lime", failed: "ember", blocked: "amber", waiting: "amber" };
const TEXT = { violet: "text-violet", lime: "text-lime", ember: "text-ember", amber: "text-amber", mute: "text-mute" };
const STATE_LABEL = { started: "Working", done: "Done", failed: "Failed", blocked: "Blocked", waiting: "Waiting" };

/** Poll /admin/agent-log, merging new and changed entries by id. */
function useAgentLog() {
  const call = useCompanyApi();
  const [byId, setById] = useState(() => new Map());
  const [error, setError] = useState(null);
  const [loaded, setLoaded] = useState(false);
  const rev = useRef(null);

  useEffect(() => {
    let stop = false;
    let timer;
    const tick = async () => {
      // Don't poll a hidden tab; catch up when it is visible again.
      if (document.visibilityState === "visible") {
        try {
          const d = await call(`/admin/agent-log${rev.current != null ? `?after=${rev.current}` : ""}`);
          if (stop) return;
          rev.current = d.rev;
          if (d.entries.length) {
            setById((prev) => {
              const next = new Map(prev);
              d.entries.forEach((e) => next.set(e.id, e));
              return next;
            });
          }
          setError(null);
          setLoaded(true);
        } catch (e) {
          if (!stop) setError(e.message);
        }
      }
      if (!stop) timer = setTimeout(tick, POLL_MS);
    };
    tick();
    return () => {
      stop = true;
      clearTimeout(timer);
    };
  }, [call]);

  // After a reply: read the whole log again, so no change made with it is missed.
  return { entries: byId, error, loaded, resync: () => (rev.current = null) };
}

/** Group entries into one session per task, most recently active first. */
function sessionsOf(entries) {
  const by = new Map();
  for (const e of entries.values()) {
    let s = by.get(e.task_id);
    if (!s) by.set(e.task_id, (s = { id: e.task_id, objective: e.task_objective, worker: e.worker_name, entries: [] }));
    s.entries.push(e);
    if (e.task_objective) s.objective = e.task_objective;
  }
  const now = Date.now();
  const out = [...by.values()].map((s) => {
    s.entries.sort((a, b) => a.created_at.localeCompare(b.created_at) || a.rev - b.rev);
    s.updated = s.entries.reduce((m, e) => (e.updated_at > m ? e.updated_at : m), "");
    const status = [...s.entries].reverse().find((e) => e.kind === "status");
    s.state = status?.detail?.state || "started";
    // An open question waits on a person, not on the worker.
    s.awaiting = s.entries.some((e) => e.kind === "question" && !e.done);
    s.live = !s.awaiting && (s.state === "started" || s.entries.some((e) => !e.done && e.kind !== "question")) && now - new Date(s.updated).getTime() < STALE_MS;
    return s;
  });
  return out.sort((a, b) => b.updated.localeCompare(a.updated));
}

export default function AgentLog() {
  const { entries, error, loaded, resync } = useAgentLog();
  const sessions = useMemo(() => sessionsOf(entries), [entries]);
  // Follow the most recently active session until the viewer picks one.
  const [picked, setPicked] = useState(null);
  const current = sessions.find((s) => s.id === picked) || sessions[0];
  const liveCount = sessions.filter((s) => s.live).length;
  const askingCount = sessions.filter((s) => s.awaiting).length;

  return (
    <>
      <PageHead kicker="What your AI workers are thinking, live" title="Agent log">
        {loaded && !error && (
          <div className="flex flex-wrap gap-2">
            {askingCount > 0 && <Badge tone="amber" dot>{askingCount} waiting for your reply</Badge>}
            <Badge tone={liveCount ? "violet" : "mute"} dot={liveCount > 0}>
              {liveCount ? `${liveCount} working now` : "No one working right now"}
            </Badge>
          </div>
        )}
      </PageHead>
      <ErrorNote error={error} />
      {!loaded && !error && <Loading rows={6} />}
      {loaded && sessions.length === 0 && (
        <Empty
          title="No agent activity yet"
          text="When an AI worker takes a task, its thinking, tool calls, and replies stream in here. Workers think with Claude once an Anthropic key is set under Integrations."
        />
      )}
      {sessions.length > 0 && (
        <div className="grid grid-cols-[minmax(0,1fr)] gap-4 xl:grid-cols-[22rem_minmax(0,1fr)]">
          <Panel className="p-1.5 xl:sticky xl:top-6 xl:self-start">
            <p className="px-3 pb-1 pt-2 font-mono text-[11px] uppercase tracking-[0.18em] text-dim">Sessions</p>
            <ul className="max-h-64 space-y-0.5 overflow-y-auto xl:max-h-[calc(100vh-10rem)]">
              {sessions.map((s) => (
                <li key={s.id}>
                  <SessionRow s={s} active={s.id === current?.id} onClick={() => setPicked(s.id)} />
                </li>
              ))}
            </ul>
          </Panel>
          {current && <Transcript key={current.id} s={current} onSent={resync} />}
        </div>
      )}
    </>
  );
}

function SessionRow({ s, active, onClick }) {
  return (
    <button
      onClick={onClick}
      aria-current={active}
      className={`flex w-full items-start gap-3 rounded-2xl px-3 py-2.5 text-left transition ${active ? "bg-white/7" : "hover:bg-white/3.5"}`}
    >
      <Avatar name={s.worker} kind="ai_agent" size={32} tag={false} />
      <span className="min-w-0 flex-1">
        <span className="line-clamp-2 text-sm text-fog">{tidy(s.objective) || "Task"}</span>
        <span className="mt-0.5 flex items-center gap-2 text-xs text-mute">
          <span className="truncate">{s.worker}</span>
          <span className="shrink-0 text-dim">· {ago(s.updated)}</span>
        </span>
      </span>
      {s.awaiting ? (
        <span className="mt-0.5 shrink-0 text-[11px] text-amber">Needs reply</span>
      ) : s.live ? (
        <span className="mt-1.5 shrink-0 rounded-full bg-violet text-violet pulse-dot size-2" title="Working now" />
      ) : (
        <span className={`mt-0.5 shrink-0 text-[11px] ${TEXT[STATE_TONE[s.state] || "mute"]}`}>{STATE_LABEL[s.state] || s.state}</span>
      )}
    </button>
  );
}

function Transcript({ s, onSent }) {
  const end = useRef(null);
  const box = useRef(null);
  // Keep the newest line in view while following along; stop if the viewer
  // scrolls up to read, and pick up again once they scroll back down.
  const stuck = useRef(true);
  const size = s.entries.reduce((n, e) => n + e.content.length, s.entries.length);
  useEffect(() => {
    if (stuck.current) end.current?.scrollIntoView({ block: "nearest" });
  }, [size]);
  const onScroll = () => {
    const el = box.current;
    if (el) stuck.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
  };

  return (
    <Panel className="flex min-w-0 flex-col p-0">
      <header className="flex flex-wrap items-start gap-3 border-b border-white/5 px-5 py-4">
        <Avatar name={s.worker} kind="ai_agent" size={40} />
        <div className="mr-auto min-w-0">
          <h2 className="font-display text-lg/snug font-semibold tracking-tight">{tidy(s.objective) || "Task"}</h2>
          <p className="mt-0.5 text-xs text-mute">{s.worker}</p>
        </div>
        <Badge tone={s.awaiting ? "amber" : s.live ? "violet" : STATE_TONE[s.state] || "mute"} dot={s.live || s.awaiting}>
          {s.awaiting ? "Needs your reply" : s.live ? "Working now" : STATE_LABEL[s.state] || s.state}
        </Badge>
      </header>
      <ol ref={box} onScroll={onScroll} className="max-h-[70vh] space-y-3 overflow-y-auto px-5 py-5" aria-live="polite">
        {s.entries.map((e) => (
          <li key={e.id}>
            <Entry e={e} />
          </li>
        ))}
        <li ref={end} aria-hidden="true" />
      </ol>
      {s.awaiting && <ReplyBox taskId={s.id} worker={s.worker} onSent={onSent} />}
    </Panel>
  );
}

/** Answer the worker's questions; it carries on with the reply. */
export function ReplyBox({ taskId, worker, onSent, autoFocus = false }) {
  const call = useCompanyApi();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(null);
  const send = async () => {
    if (!text.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      await call(`/admin/tasks/${taskId}/reply`, { method: "POST", body: { text } });
      setText("");
      onSent?.();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="border-t border-white/5 px-5 py-4"
      onSubmit={(e) => {
        e.preventDefault();
        send();
      }}
    >
      <label htmlFor={`reply-${taskId}`} className="mb-2 block text-xs text-mute">
        Reply to {worker}. It carries on with your answer right away.
      </label>
      <textarea
        id={`reply-${taskId}`}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          // Enter sends; Shift+Enter starts a new line.
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            send();
          }
        }}
        rows={3}
        maxLength={8000}
        autoFocus={autoFocus}
        placeholder="Your answer…"
        className="w-full resize-y rounded-xl bg-black/30 px-3 py-2 text-sm text-fog ring-1 ring-white/10 placeholder:text-dim focus:outline-none focus:ring-lime/50"
      />
      <ErrorNote error={error} />
      <div className="mt-2 flex items-center gap-3">
        <span className="mr-auto text-[11px] text-dim">Enter to send · Shift+Enter for a new line</span>
        <Button size="sm" type="submit" disabled={busy || !text.trim()}>{busy ? "Sending…" : "Send reply"}</Button>
      </div>
    </form>
  );
}

function Entry({ e }) {
  if (e.kind === "status") {
    const tone = STATE_TONE[e.detail?.state] || "mute";
    return (
      <div className="flex items-center gap-3 py-1 text-xs">
        <span className="h-px flex-1 bg-white/8" />
        <span className={`max-w-[80%] text-center ${TEXT[tone]}`}>{tidy(e.content)}</span>
        <span className="shrink-0 text-dim" title={when(e.created_at)}>{ago(e.created_at)}</span>
        <span className="h-px flex-1 bg-white/8" />
      </div>
    );
  }
  if (e.kind === "thinking") {
    return (
      <div className="rounded-2xl border-l-2 border-violet/50 bg-violet/5 px-4 py-3">
        <p className="mb-1 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-widest text-violet">
          <Icon name="bolt" size={12} /> {e.done ? "Thought" : "Thinking"}
        </p>
        <p className={`whitespace-pre-wrap text-sm/relaxed text-fog/75 ${e.done ? "" : "caret"}`}>{e.content}</p>
      </div>
    );
  }
  if (e.kind === "question") {
    return (
      <div className="rounded-2xl bg-amber/8 px-4 py-3 ring-1 ring-amber/30">
        <p className="mb-1 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-widest text-amber">
          <Icon name="user" size={12} /> {e.done ? "Asked you" : "Question for you"}
        </p>
        <p className="whitespace-pre-wrap text-sm/relaxed text-fog">{e.content}</p>
      </div>
    );
  }
  if (e.kind === "reply") {
    return (
      <div className="ml-auto max-w-[85%] rounded-2xl rounded-br-md bg-lime/10 px-4 py-3 ring-1 ring-lime/25">
        <p className="mb-1 text-[11px] text-lime">{e.detail?.from || "You"} replied · <span title={when(e.created_at)}>{ago(e.created_at)}</span></p>
        <p className="whitespace-pre-wrap text-sm/relaxed text-fog">{e.content}</p>
      </div>
    );
  }
  if (e.kind === "tool") {
    const outcome = e.detail?.outcome;
    const args = e.detail?.arguments;
    const hasArgs = args && typeof args === "object" && Object.keys(args).length > 0;
    return (
      <details className="group rounded-xl bg-white/3 ring-1 ring-white/8">
        <summary className="flex cursor-pointer list-none items-center gap-2 px-3 py-2 text-xs [&::-webkit-details-marker]:hidden">
          <Icon name="tools" size={14} className="shrink-0 text-mute" />
          <span className="truncate font-mono text-violet">{e.detail?.tool || e.content}</span>
          <Badge tone={outcome === "ok" ? "lime" : outcome === "error" ? "ember" : "amber"}>{outcome === "ok" ? "ran" : outcome || "?"}</Badge>
          {e.content !== e.detail?.tool && <span className="min-w-0 truncate text-mute">{tidy(e.content.replace(`${e.detail?.tool}: `, ""))}</span>}
          {hasArgs && <Icon name="chevronRight" size={14} className="ml-auto shrink-0 text-dim transition group-open:rotate-90" />}
        </summary>
        {hasArgs && <pre className="overflow-x-auto border-t border-white/5 px-3 py-2 font-mono text-[11px]/relaxed text-mute">{JSON.stringify(args, null, 2)}</pre>}
      </details>
    );
  }
  // Reply text: notes between tool calls, and the final report.
  return <p className={`whitespace-pre-wrap text-sm/relaxed text-fog ${e.done ? "" : "caret"}`}>{e.content}</p>;
}
