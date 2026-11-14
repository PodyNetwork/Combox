// At a glance: payouts waiting, the monthly cap, the kill switch, the
// company's wallet (NEAR treasury) and its yield on Rhea, the team, the task pipeline, spot
// checks, interviews coming up, and the latest feed.
import { TreasuryCard } from "../components/Treasury.jsx";
import { TreasuryYieldCard } from "../components/TreasuryYield.jsx";
import { Badge, CountUp, Empty, ErrorNote, Eyebrow, Icon, KineticText, Loading, Orb, Panel, PanelHead, STATUS_TONE, ago, day, human, pctOf, tidy, useApp, useLoad, useThreshold, usd } from "../ui.jsx";

const TASK_ORDER = ["queued", "assigned", "in_progress", "blocked", "done", "failed"];
// One neutral scale for work in flight; lime only for finished work.
const BAR = { queued: "bg-white/25", assigned: "bg-white/40", in_progress: "bg-white/60", blocked: "bg-white/40", done: "bg-lime", failed: "bg-white/40" };
const LABEL = { queued: "Queued", assigned: "Assigned", in_progress: "In progress", blocked: "Blocked", done: "Done", failed: "Failed" };
const CLOSED = ["done", "failed", "cancelled"];
const STATUS_ROWS = [
  ["active", "Active", "bg-lime"],
  ["on_probation", "Probation", "bg-amber"],
  ["candidate", "Candidate", "bg-sky"],
  ["suspended", "Suspended", "bg-ember"],
  ["fired", "Fired", "bg-ember"],
];
// Small text link used in panel headers.
const LINK = "inline-flex items-center gap-1 text-xs text-mute transition hover:text-fog";

export default function Overview() {
  const { platform: isPlatform, company, navigate, status } = useApp();
  const { kill, pending, feed } = status;
  const cap = useLoad("/admin/cap");
  const org = useLoad("/org-chart");
  const tasks = useLoad("/tasks");
  const meetings = useLoad("/admin/meetings");
  const spot = useLoad("/spot-checks/stats");
  const threshold = useThreshold();

  const workers = (org.data?.departments || []).flatMap((d) => d.teams.flatMap((t) => t.workers));
  const noPlan = !!org.error && /no plan/i.test(org.error);
  const halted = !!kill.data?.on;
  const held = pending.data?.payouts.length;
  const sub = company?.subscription;
  const hour = new Date().getHours();
  const hello = hour < 5 ? "Up late." : hour < 12 ? "Good morning." : hour < 18 ? "Good afternoon." : "Good evening.";

  return (
    <div className="space-y-4">
      <div className="mb-6 flex flex-wrap items-end gap-4">
        <h1 className="mr-auto font-display text-3xl/tight font-semibold tracking-tight sm:text-4xl">
          <KineticText text={hello} />{" "}
          <span className={halted || held ? "text-ember" : "text-lime"}>
            <KineticText
              key={`${halted}-${held}`}
              delay={14}
              text={halted ? "Everything is halted." : held == null ? (pending.error ? "Payouts are unavailable right now." : "Checking the books…") : held ? `${held} payout${held > 1 ? "s" : ""} need${held > 1 ? "" : "s"} you.` : "Nothing needs you."}
            />
          </span>
        </h1>
        {isPlatform && sub && <Subscription sub={sub} onOpen={() => navigate("billing")} />}
      </div>
      {isPlatform && noPlan && (
        <p className="reveal rounded-2xl bg-sky/10 px-4 py-3 text-sm text-sky ring-1 ring-sky/30">
          No plan yet. Your org chart and tasks appear once you set your company's goal (Settings, or create a new company to use the setup guide).
        </p>
      )}
      <div className="grid gap-4 md:grid-cols-6 xl:grid-cols-12">
        <Hero cap={cap} halted={halted} onOpen={() => navigate("spending")} />
        <ApprovalsCard pending={pending} threshold={threshold} onOpen={() => navigate("approvals")} />
        <KillCard state={kill} onOpen={() => navigate("kill")} />
        <TreasuryCard i={3} className="md:col-span-3 xl:col-span-4" />
        <TreasuryYieldCard i={4} className="md:col-span-3 xl:col-span-4" />
        <Workforce workers={workers} loading={org.loading} error={noPlan ? null : org.error} noPlan={noPlan} onOpen={() => navigate("team")} />
        <Pipeline tasks={tasks.data?.tasks} error={tasks.error} onOpen={() => navigate("tasks")} />
        <SpotChecks data={spot.data} error={spot.error} />
        <UpNext meetings={meetings.data?.meetings} error={meetings.error} onOpen={() => navigate("meetings")} />
        <Latest feed={feed} onOpen={() => navigate("activity")} />
      </div>
    </div>
  );
}

// Platform only: where the company's subscription stands.
function Subscription({ sub, onOpen }) {
  const value = sub.state === "trial" ? "Free trial" : sub.state === "active" ? "Active" : "Ended";
  const caption = sub.state === "trial" ? `Ends ${day(sub.ends_at)}` : sub.state === "active" ? `Paid until ${day(sub.paid_until)}` : `Since ${day(sub.since)}`;
  const tone = sub.state === "expired" ? "ember" : sub.state === "active" ? "lime" : "mute";
  return (
    <button onClick={onOpen} className="reveal flex items-center gap-3 rounded-2xl bg-white/4 px-4 py-2.5 text-left ring-1 ring-white/10 transition hover:bg-white/6">
      <span>
        <span className="block font-mono text-[10px] uppercase tracking-[0.18em] text-dim">Subscription</span>
        <span className="mt-0.5 flex items-center gap-2 text-sm">
          <Badge tone={tone} dot={tone !== "mute"}>{value}</Badge>
          <span className={sub.state === "expired" ? "text-ember" : "text-mute"}>{caption}</span>
        </span>
      </span>
      <Icon name="arrow" size={14} className="text-dim" />
    </button>
  );
}

function Hero({ cap, halted, onOpen }) {
  const d = cap.data;
  const used = d ? Number(d.committed_usd) + Number(d.reserved_usd) : 0;
  const pct = d ? pctOf(used, d.cap_usd) : 0;
  // Same warning levels as before: amber past 70%, ember past 90%.
  const pctTone = pct > 90 ? "text-ember" : pct > 70 ? "text-amber" : "text-fog";
  return (
    <Panel i={0} className="min-h-88 p-6 sm:p-8 md:col-span-6 xl:col-span-8 xl:row-span-2">
      <Orb halted={halted} className="pointer-events-none absolute -right-16 -top-10 w-72 opacity-70 sm:-right-10 sm:w-96 xl:w-md" />
      <div className="relative flex h-full flex-col">
        <Eyebrow>Monthly payout cap · {d?.month ? new Date(`${String(d.month).slice(0, 7)}-01T00:00:00Z`).toLocaleString(undefined, { month: "long", year: "numeric", timeZone: "UTC" }) : "this month"} UTC</Eyebrow>
        <p className="mt-6 text-sm text-mute">Left to pay out this month</p>
        {cap.error ? (
          <div className="mt-2 max-w-md"><ErrorNote error={cap.error} /></div>
        ) : (
          <p className="num mt-1 font-display text-6xl font-semibold leading-none tracking-tighter sm:text-8xl">
            {d ? <CountUp value={d.remaining_usd} format={(v) => usd(v, 0)} /> : <span className="shimmer inline-block h-20 w-64 max-w-full rounded-2xl" />}
          </p>
        )}
        <p className="mt-3 text-sm text-mute">
          left of a <span className="text-fog">{d ? usd(d.cap_usd) : "…"}</span> cap that can never be exceeded, even by an approval
        </p>
        <div className="mt-auto pt-10">
          {d && <p className="mb-2 text-xs text-mute"><span className={`num font-medium ${pctTone}`}>{Math.round(pct)}%</span> used or reserved</p>}
          <div className="flex h-3 w-full overflow-hidden rounded-full bg-white/5" role="img" aria-label={`Cap ${pct.toFixed(0)}% used`}>
            {d && (
              <>
                <div className="grow-x bg-lime" style={{ width: `${pctOf(d.committed_usd, d.cap_usd)}%` }} />
                <div className="grow-x bg-amber" style={{ width: `${pctOf(d.reserved_usd, d.cap_usd)}%`, "--i": 2 }} />
              </>
            )}
          </div>
          <div className="mt-3 flex flex-wrap items-center gap-x-6 gap-y-1 text-xs text-mute">
            <span><i className="mr-1.5 inline-block rounded-full bg-lime size-2" />Sent <span className="num text-fog">{d ? usd(d.committed_usd) : "…"}</span></span>
            <span><i className="mr-1.5 inline-block rounded-full bg-amber size-2" />Reserved <span className="num text-fog">{d ? usd(d.reserved_usd) : "…"}</span></span>
            <button onClick={onOpen} className="ml-auto inline-flex items-center gap-1 text-fog hover:text-lime">Cap details <Icon name="arrow" size={14} /></button>
          </div>
        </div>
      </div>
    </Panel>
  );
}

function ApprovalsCard({ pending, threshold, onOpen }) {
  const list = pending.data?.payouts || [];
  const total = list.reduce((s, p) => s + Number(p.amount_usd), 0);
  const hot = list.length > 0;
  return (
    <Panel
      i={1}
      className={`flex flex-col p-6 md:col-span-3 xl:col-span-4 ${
        hot
          ? "border-ember/40 bg-[radial-gradient(26rem_16rem_at_100%_0%,rgba(255,90,31,0.22),transparent_65%)]"
          : "border-lime/30 bg-[radial-gradient(26rem_16rem_at_100%_0%,rgba(212,255,58,0.12),transparent_65%)]"
      }`}
    >
      <p className={`flex items-center gap-2 font-mono text-[11px] uppercase tracking-[0.18em] ${hot ? "text-ember" : "text-lime"}`}>
        <span className={`pulse-dot rounded-full size-1.5 ${hot ? "bg-ember" : "bg-lime"}`} />
        {hot ? "Waiting for your approval" : "All clear"}
      </p>
      <ErrorNote error={pending.error} />
      <div className="mt-4 flex items-end gap-3">
        <span className="num font-display text-7xl font-bold leading-none tracking-tighter text-fog">{pending.data ? <CountUp value={list.length} /> : "–"}</span>
        <span className="mb-1.5 leading-tight">
          <span className="block text-sm text-fog">payout{list.length === 1 ? "" : "s"} waiting</span>
          {hot && <span className="num block text-lg font-semibold text-ember">{usd(total)}</span>}
        </span>
      </div>
      <p className="mt-4 text-sm/relaxed text-mute">
        Payouts under {usd(threshold)} send automatically; {usd(threshold)} or more wait for you.
        {hot ? " Approve with MFA, or reject with a reason." : " Nothing waiting."}
      </p>
      <div className="mt-auto pt-6">
        <button
          onClick={onOpen}
          className={`inline-flex items-center gap-2 rounded-full px-4 py-2 text-sm font-semibold transition ${
            hot ? "bg-ember text-ink hover:bg-[#ff7a45]" : "bg-white/5 text-fog ring-1 ring-white/10 hover:bg-white/10"
          }`}
        >
          {hot ? "Review approvals" : "Open payroll"} <Icon name="arrow" size={16} />
        </button>
      </div>
    </Panel>
  );
}

function KillCard({ state, onOpen }) {
  const on = !!state.data?.on;
  return (
    <Panel i={2} as="button" onClick={onOpen} className="group flex items-center gap-4 p-6 text-left md:col-span-3 xl:col-span-4" aria-label={`Kill switch ${on ? "on" : "off"}. Open the kill switch`}>
      <div className={`relative h-14 w-24 shrink-0 rounded-full p-1.5 transition ${on ? "hazard" : "bg-white/10"}`}>
        <span className={`block rounded-full shadow-xl transition-all duration-500 size-11 ${on ? "translate-x-[2.4rem] bg-ink ring-2 ring-ember" : "bg-lime"}`} />
      </div>
      <div className="min-w-0">
        <Eyebrow>Kill switch</Eyebrow>
        <p className={`mt-1 font-display text-xl font-semibold ${on ? "text-ember" : ""}`}>{state.data ? (on ? "ON · HALTED" : "Off") : state.error ? "Unknown" : "…"}</p>
        {on && <p className="text-xs text-ember/90">All tool calls and payouts are halted.</p>}
        {state.data && <p className="truncate text-xs text-mute">Changed {ago(state.data.updated_at)} by {state.data.updated_by}</p>}
        {state.error && <p className="text-xs text-ember">{state.error}</p>}
      </div>
    </Panel>
  );
}

function Workforce({ workers, loading, error, noPlan, onOpen }) {
  // Active means working: active or on probation.
  const active = workers.filter((w) => w.status === "active" || w.status === "on_probation");
  const ai = workers.filter((w) => w.kind === "ai_agent");
  const people = workers.filter((w) => w.kind !== "ai_agent");
  const byStatus = workers.reduce((m, w) => ({ ...m, [w.status]: (m[w.status] || 0) + 1 }), {});
  return (
    <Panel i={3} className="p-6 md:col-span-3 xl:col-span-4">
      <PanelHead
        eyebrow={`Workforce · ${workers.length} in total`}
        title={loading ? "Active workers" : `${active.length} active worker${active.length === 1 ? "" : "s"}`}
        action={<button onClick={onOpen} className={LINK}>Org chart <Icon name="arrow" size={12} /></button>}
      />
      <ErrorNote error={error} />
      {noPlan ? (
        <Empty title="No plan yet" text="Workers appear once the company has a plan." />
      ) : loading ? (
        <Loading rows={2} />
      ) : (
        !error && (
          <>
            {/* AI vs people split */}
            <div className="flex items-end gap-6">
              <div>
                <p className="num font-display text-3xl font-semibold leading-none text-violet"><CountUp value={ai.length} /></p>
                <p className="mt-1.5 text-xs text-mute">AI agents</p>
              </div>
              <div className="h-10 w-px bg-white/10" />
              <div>
                <p className="num font-display text-3xl font-semibold leading-none text-sky"><CountUp value={people.length} /></p>
                <p className="mt-1.5 text-xs text-mute">People</p>
              </div>
            </div>
            <div className="mt-4 flex h-1.5 overflow-hidden rounded-full bg-white/6" role="img" aria-label={`${ai.length} AI agents, ${people.length} people`}>
              <div className="grow-x h-full bg-violet" style={{ width: `${pctOf(ai.length, workers.length)}%` }} />
              <div className="grow-x h-full bg-sky" style={{ width: `${pctOf(people.length, workers.length)}%`, "--i": 2 }} />
            </div>

            {/* Status breakdown: colour only on the dot */}
            <ul className="mt-5 grid grid-cols-2 gap-x-4 gap-y-2 border-t border-white/5 pt-4 text-sm">
              {STATUS_ROWS.map(([s, name, dot]) => (
                <li key={s} className={`flex items-center gap-2 ${byStatus[s] ? "" : "opacity-40"}`}>
                  <span className={`shrink-0 rounded-full size-1.5 ${dot}`} />
                  <span className="text-mute">{name}</span>
                  <span className="num ml-auto text-fog">{byStatus[s] || 0}</span>
                </li>
              ))}
            </ul>
          </>
        )
      )}
    </Panel>
  );
}

// Horizontal rows read well at any card width.
function Pipeline({ tasks, error, onOpen }) {
  const counts = TASK_ORDER.map((s) => [s, (tasks || []).filter((t) => t.status === s).length]);
  const max = Math.max(1, ...counts.map(([, n]) => n));
  const open = (tasks || []).filter((t) => !CLOSED.includes(t.status));
  return (
    <Panel i={4} className="p-6 md:col-span-3 xl:col-span-4">
      <PanelHead
        eyebrow={`Task pipeline · ${tasks ? tasks.length : "…"} in total`}
        title={tasks ? `${open.length} open task${open.length === 1 ? "" : "s"}` : "Open tasks"}
        action={<button onClick={onOpen} className={LINK}>Board <Icon name="arrow" size={12} /></button>}
      />
      <ErrorNote error={error} />
      <ul className="space-y-2.5">
        {counts.map(([s, n], i) => (
          <li key={s} className="grid grid-cols-[5.5rem_minmax(0,1fr)_2rem] items-center gap-3 text-xs">
            <span className="flex items-center gap-1.5 text-mute">
              {LABEL[s]}
              {(s === "blocked" || s === "failed") && n > 0 && <span className={`size-1.5 rounded-full ${s === "failed" ? "bg-ember" : "bg-amber"}`} title="Needs attention" />}
            </span>
            <span className="h-2 overflow-hidden rounded-full bg-white/5">
              <span className={`grow-x block h-full rounded-full ${BAR[s]}`} style={{ width: `${tasks ? (n / max) * 100 : 0}%`, "--i": i }} />
            </span>
            <span className={`num text-right ${n ? "text-fog" : "text-dim"}`}>{tasks ? n : "–"}</span>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

function SpotChecks({ data, error }) {
  const rate = data?.disagreement_rate;
  const agree = rate == null ? null : 1 - rate;
  const C = 2 * Math.PI * 42;
  return (
    <Panel i={5} className="flex flex-col p-6 md:col-span-3 xl:col-span-4">
      <PanelHead eyebrow="Spot checks" title="Owner agreement" />
      <ErrorNote error={error} />
      <div className="flex items-center gap-5">
        <svg viewBox="0 0 100 100" className="shrink-0 -rotate-90 size-28" role="img" aria-label={agree == null ? "No answers yet" : `${Math.round(agree * 100)}% agreement`}>
          <circle cx="50" cy="50" r="42" fill="none" stroke="rgb(255 255 255 / 0.06)" strokeWidth="10" />
          {agree != null && <circle cx="50" cy="50" r="42" fill="none" stroke="var(--color-lime)" strokeWidth="10" strokeLinecap="round" strokeDasharray={`${agree * C} ${C}`} style={{ transition: "stroke-dasharray 1s" }} />}
        </svg>
        <div className="min-w-0">
          <p className="num font-display text-4xl font-semibold">{agree == null ? "—" : `${Math.round(agree * 100)}%`}</p>
          <p className="text-xs text-mute">
            {!data ? "…" : data.answered ? `You agreed with the reviewer agent on ${data.answered - data.disagreements} of ${data.answered} sampled reviews.` : "No sampled reviews answered yet."}
          </p>
          <p className="mt-1 text-xs text-dim">{data?.reviews ?? "…"} reviews · {data?.sampled ?? "…"} sampled</p>
        </div>
      </div>
    </Panel>
  );
}

function UpNext({ meetings, error, onOpen }) {
  const upcoming = (meetings || [])
    .filter((m) => ["scheduled", "dispatched"].includes(m.status))
    .sort((a, b) => new Date(a.scheduled_at) - new Date(b.scheduled_at))
    .slice(0, 3);
  return (
    <Panel i={6} className="p-6 md:col-span-3 xl:col-span-4">
      <PanelHead eyebrow="Interviews" title="Up next" action={<button onClick={onOpen} className={LINK}>All <Icon name="arrow" size={12} /></button>} />
      <ErrorNote error={error} />
      {!meetings && !error && <Loading rows={2} />}
      {meetings && upcoming.length === 0 && <Empty title="No interviews booked" text="Create one from the Interviews page." />}
      <ul className="space-y-2">
        {upcoming.map((m) => {
          const d = new Date(m.scheduled_at);
          return (
            <li key={m.id}>
              <button onClick={onOpen} className="flex w-full items-center gap-3 rounded-2xl bg-white/3 p-3 text-left transition hover:bg-white/6">
                <span className="flex shrink-0 flex-col items-center justify-center rounded-xl bg-white/5 leading-none size-12">
                  <span className="font-mono text-[9px] uppercase text-mute">{d.toLocaleString(undefined, { month: "short" })}</span>
                  <span className="font-display text-lg font-semibold">{d.getDate()}</span>
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm">{m.title}</span>
                  <span className="block truncate text-xs text-mute">{d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })} · {ago(m.scheduled_at)}</span>
                </span>
                <Badge tone={STATUS_TONE[m.status]} dot={m.status === "dispatched"}>{human(m.status)}</Badge>
              </button>
            </li>
          );
        })}
      </ul>
    </Panel>
  );
}

function Latest({ feed, onOpen }) {
  const list = (feed.data?.notifications || []).slice(0, 4);
  return (
    <Panel i={7} className="p-6 md:col-span-3 xl:col-span-4">
      <PanelHead
        eyebrow="Owner feed"
        title="Latest decisions"
        action={<button onClick={onOpen} className="inline-flex items-center rounded-full bg-white/5 px-3 py-1.5 text-xs font-medium text-fog ring-1 ring-inset ring-white/10 transition hover:bg-white/10">Open feed</button>}
      />
      <ErrorNote error={feed.error} />
      {feed.loading && <Loading />}
      {feed.data && list.length === 0 && <Empty title="Quiet so far" />}
      <ul className="divide-y divide-white/5">
        {list.map((n) => (
          <li key={n.id} className="flex items-start gap-3 py-3">
            <span className={`mt-1.5 shrink-0 rounded-full size-2 ${/fired|blocked|failed|rejected|exceeded|kill_switch/.test(n.kind) ? "bg-ember" : /held|probation|pending/.test(n.kind) ? "bg-amber" : "bg-lime"}`} />
            <div className="min-w-0 flex-1">
              <p className="text-sm">{tidy(n.subject)}</p>
              {n.body?.reason && <p className="truncate text-xs text-mute">{tidy(n.body.reason)}</p>}
            </div>
            <span className="shrink-0 font-mono text-[11px] text-dim" title={new Date(n.created_at).toLocaleString()}>{ago(n.created_at)}</span>
          </li>
        ))}
      </ul>
    </Panel>
  );
}
