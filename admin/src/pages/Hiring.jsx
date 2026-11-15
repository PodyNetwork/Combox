// Hiring: the company's public jobs board and how each applicant moves
// through screening, the AI interview, and the contract, with a progress log
// per application and the reason whenever someone is out of the running.
import { useMemo, useRef, useState } from "react";
import { download } from "../api.js";
import {
  Avatar, Badge, Button, Chip, CopyField, CountUp, Empty, ErrorNote, Eyebrow, Icon, Loading, PageHead, Panel, PanelHead,
  ago, day, human, useApp, useLoad, useSiteUrl, usd, when,
} from "../ui.jsx";

const STEPS = [
  ["Screen", "New applicants are checked against the job's skills within a minute. What they write is stored as data, never acted on."],
  ["Interview", "The top three skill matches get an email invite to a video interview with your AI interviewer, which asks consent first."],
  ["Contract", "The best candidate scoring 3.5/5 or more gets a contract from your approved DocuSeal template."],
  ["Onboard", "Once signed, they get their task briefs. Before the first payout they verify a NEAR address and submit a W-9 or W-8BEN."],
];

const STEP_LABELS = ["Applied", "Screened", "Interview", "Scored", "Contract", "Hired"];

// Filters over `progress.outcome`.
const FILTERS = [
  { value: "all", label: "All", test: () => true },
  { value: "active", label: "In progress", test: (o) => o === "in_progress" },
  { value: "not_qualified", label: "Not qualified", test: (o) => o === "not_qualified" },
  { value: "out", label: "Not selected", test: (o) => o === "not_selected" || o === "withdrew" || o === "closed" },
  { value: "hired", label: "Hired", test: (o) => o === "hired" },
];

// Badge colour for where an application stands.
function tone(p) {
  if (!p) return "mute";
  if (p.state === "done") return "lime";
  if (p.outcome === "not_qualified") return "ember";
  if (p.state === "stopped") return "mute";
  if (p.state === "working") return "violet";
  return "amber";
}

export default function Hiring() {
  const { platform: isPlatform, navigate } = useApp();
  const jobs = useLoad("/jobs");
  const apps = useLoad("/admin/applications", { poll: 15000 });
  const boardUrl = useSiteUrl()("/jobs");
  const list = jobs.data?.jobs || [];
  const applications = apps.data?.applications || [];
  const [jobFilter, setJobFilter] = useState("all");
  const applicantsRef = useRef(null);

  const byJob = useMemo(() => {
    const m = {};
    for (const a of applications) {
      const s = (m[a.job_id] ||= { total: 0, active: 0, unqualified: 0 });
      s.total++;
      if (a.progress?.outcome === "in_progress") s.active++;
      if (a.progress?.outcome === "not_qualified") s.unqualified++;
    }
    return m;
  }, [applications]);

  const showJob = (id) => {
    setJobFilter(id);
    applicantsRef.current?.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  return (
    <>
      <PageHead kicker="Jobs board · AI interviews" title="Hiring">
        <a
          href={boardUrl}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center gap-2 rounded-full bg-white/5 px-4 py-2.5 text-sm font-medium text-fog ring-1 ring-inset ring-white/10 transition hover:bg-white/10"
        >
          View board <Icon name="external" size={15} />
        </a>
      </PageHead>
      <p className="reveal -mt-2 mb-6 max-w-2xl text-sm text-mute">When no one on the team can do a task, Combox posts a job on your board and runs the hiring for you. Every applicant and where they stand is below.</p>

      <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,1fr)_22rem]">
        <div className="min-w-0 space-y-4">
          <div ref={applicantsRef} className="scroll-mt-6">
            <Applicants apps={apps} jobFilter={jobFilter} setJobFilter={setJobFilter} />
          </div>

          <Panel i={1} className="p-6">
            <PanelHead
              eyebrow="On your board now"
              title="Open jobs"
              action={
                jobs.data && (
                  <span className="num font-display text-4xl font-semibold leading-none text-lime">
                    <CountUp value={list.length} />
                  </span>
                )
              }
            />
            <ErrorNote error={jobs.error} />
            {jobs.loading && <Loading rows={3} />}
            {jobs.data && list.length === 0 && <Empty title="No open jobs" text="They appear when a task needs someone you don't have." />}
            {list.length > 0 && (
              <ul className="space-y-3">
                {list.map((j, i) => {
                  const s = byJob[j.id] || { total: 0, active: 0, unqualified: 0 };
                  return (
                    <li key={j.id} className="reveal rounded-2xl bg-white/3 p-4 ring-1 ring-white/5 transition hover:bg-white/4" style={{ "--i": i }}>
                      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                        <h3 className="mr-auto font-medium">{j.title}</h3>
                        <p className="text-sm">
                          <span className="num font-display text-lg font-semibold">{usd(j.rate_usd)}</span>
                          <span className="text-xs text-mute"> per task</span>
                        </p>
                      </div>
                      <p className="mt-0.5 text-xs text-dim" title={new Date(j.created_at).toLocaleString()}>
                        Posted {day(j.created_at)} · {ago(j.created_at)}
                      </p>
                      {j.skills?.length > 0 && (
                        <div className="mt-3 flex flex-wrap gap-1">{j.skills.map((sk) => <Chip key={sk}>{human(sk)}</Chip>)}</div>
                      )}
                      <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-2 border-t border-white/5 pt-3 text-xs">
                        <span className="text-fog"><span className="num font-semibold">{s.total}</span> <span className="text-mute">applied</span></span>
                        <span><span className="num font-semibold text-violet">{s.active}</span> <span className="text-mute">in progress</span></span>
                        <span><span className="num font-semibold text-ember">{s.unqualified}</span> <span className="text-mute">not qualified</span></span>
                        {s.total > 0 && (
                          <Button size="sm" variant="ghost" className="ml-auto" onClick={() => showJob(j.id)}>
                            See applicants <Icon name="arrow" size={13} />
                          </Button>
                        )}
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </Panel>
        </div>

        <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-1">
          <Panel i={1} className="p-6">
            <PanelHead eyebrow="Public" title="Your jobs board" />
            <p className="-mt-2 mb-4 text-sm text-mute">Share it anywhere. Anyone can apply; no account needed.</p>
            <CopyField label="Jobs board link" value={boardUrl} />
          </Panel>

          <Panel i={2} className="p-6">
            <PanelHead eyebrow="Hands off" title="How it works" />
            <div className="relative">
              <span className="absolute bottom-2 left-3.5 top-2 w-px bg-white/10" aria-hidden="true" />
              <ol className="space-y-5">
                {STEPS.map(([name, text], i) => (
                  <li key={name} className="relative flex gap-4">
                    <span className="z-10 flex shrink-0 items-center justify-center rounded-full bg-ink font-mono text-[11px] text-lime ring-1 ring-lime/40 size-7">
                      {String(i + 1).padStart(2, "0")}
                    </span>
                    <div className="min-w-0 pt-0.5">
                      <p className="font-display text-sm font-medium">{name}</p>
                      <p className="mt-1 text-sm text-mute">{text}</p>
                    </div>
                  </li>
                ))}
              </ol>
            </div>
          </Panel>

          {isPlatform && (
            <Panel i={3} className="p-5">
              <Eyebrow className="text-sky opacity-100">Upwork</Eyebrow>
              <p className="mt-2 text-sm text-mute">To post on Upwork instead, set the marketplace in Integrations. Upwork hires are paid through Upwork, never in NEAR.</p>
              <Button variant="ghost" size="sm" className="mt-4" onClick={() => navigate("integrations")}>
                Open Integrations <Icon name="arrow" size={14} />
              </Button>
            </Panel>
          )}
        </div>
      </div>
    </>
  );
}

// Everyone who applied on the jobs board, newest first, filterable by
// outcome and job. Names and notes are what applicants typed: plain text only.
function Applicants({ apps, jobFilter, setJobFilter }) {
  const [filter, setFilter] = useState("all");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(null);
  const list = apps.data?.applications || [];

  const jobs = useMemo(() => {
    const m = new Map();
    for (const a of list) if (!m.has(a.job_id)) m.set(a.job_id, a.job_title);
    return [...m];
  }, [list]);

  const inJob = jobFilter === "all" ? list : list.filter((a) => a.job_id === jobFilter);
  const counts = Object.fromEntries(FILTERS.map((f) => [f.value, inJob.filter((a) => f.test(a.progress?.outcome)).length]));
  const q = query.trim().toLowerCase();
  const shown = inJob.filter(
    (a) => FILTERS.find((f) => f.value === filter).test(a.progress?.outcome) && (!q || a.name.toLowerCase().includes(q) || a.email.toLowerCase().includes(q)),
  );

  return (
    <Panel className="p-6">
      <PanelHead
        eyebrow="From your jobs board"
        title="Applicants"
        action={apps.data && <span className="num font-display text-4xl font-semibold leading-none text-sky"><CountUp value={list.length} /></span>}
      />
      <ErrorNote error={apps.error} />
      {apps.loading && <Loading rows={3} />}
      {apps.data && list.length === 0 && <Empty title="No applicants yet" text="Applications and CVs appear here as people apply on your board." />}

      {list.length > 0 && (
        <>
          <div className="mb-4 flex flex-wrap items-center gap-2">
            <div role="group" aria-label="Filter by outcome" className="scrollbar-none -mx-1 flex max-w-full gap-1 overflow-x-auto px-1">
              {FILTERS.map((f) => (
                <button
                  key={f.value}
                  type="button"
                  aria-pressed={filter === f.value}
                  onClick={() => setFilter(f.value)}
                  className={`inline-flex shrink-0 items-center gap-1.5 rounded-full px-3 py-1.5 text-xs font-medium ring-1 ring-inset transition ${
                    filter === f.value ? "bg-lime text-ink ring-lime" : "bg-white/3 text-mute ring-white/10 hover:text-fog"
                  }`}
                >
                  {f.label}
                  <span className={`num rounded-full px-1.5 text-[10px] ${filter === f.value ? "bg-ink/15" : "bg-white/5"}`}>{counts[f.value]}</span>
                </button>
              ))}
            </div>
            <div className="ml-auto flex w-full flex-wrap gap-2 sm:w-auto">
              {jobs.length > 1 && (
                <select
                  aria-label="Filter by job"
                  value={jobFilter}
                  onChange={(e) => setJobFilter(e.target.value)}
                  className="min-w-0 flex-1 rounded-full border border-white/10 bg-black/40 px-3 py-1.5 text-xs text-fog focus:border-lime/60 focus:outline-none sm:max-w-[14rem] sm:flex-none"
                >
                  <option value="all">All jobs</option>
                  {jobs.map(([id, title]) => (
                    <option key={id} value={id}>{title}</option>
                  ))}
                </select>
              )}
              <label className="relative min-w-0 flex-1 sm:w-44 sm:flex-none">
                <span className="sr-only">Search applicants</span>
                <Icon name="search" size={13} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-dim" />
                <input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="Name or email"
                  className="w-full rounded-full border border-white/10 bg-black/40 py-1.5 pl-8 pr-3 text-xs text-fog placeholder:text-dim focus:border-lime/60 focus:outline-none"
                />
              </label>
            </div>
          </div>
          {jobFilter !== "all" && (
            <p className="-mt-1 mb-3 flex items-center gap-2 text-xs text-mute">
              Showing applicants for <span className="text-fog">{jobs.find(([id]) => id === jobFilter)?.[1] || "this job"}</span>
              <button type="button" className="text-dim underline-offset-2 hover:text-fog hover:underline" onClick={() => setJobFilter("all")}>Show all jobs</button>
            </p>
          )}

          {shown.length === 0 ? (
            <p className="rounded-2xl bg-white/3 px-4 py-6 text-center text-sm text-mute">No applicants match.</p>
          ) : (
            <ul className="space-y-2">
              {shown.map((a) => (
                <ApplicantRow key={a.id} a={a} open={open === a.id} onToggle={() => setOpen(open === a.id ? null : a.id)} />
              ))}
            </ul>
          )}
        </>
      )}
    </Panel>
  );
}

function ApplicantRow({ a, open, onToggle }) {
  const p = a.progress || {};
  const stopped = p.state === "stopped";
  return (
    <li className={`rounded-2xl ring-1 transition ${open ? "bg-white/4 ring-white/10" : "bg-white/2 ring-white/5 hover:bg-white/4"}`}>
      <button type="button" onClick={onToggle} aria-expanded={open} className="flex w-full items-start gap-3 p-4 text-left">
        <Avatar name={a.name} size={36} />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <p className="truncate font-medium">{a.name}</p>
            <Badge tone={tone(p)} dot={p.state === "working"}>{p.headline || human(a.status)}</Badge>
          </div>
          <p className="mt-0.5 truncate text-xs text-dim">
            {a.job_title} · <span title={new Date(a.created_at).toLocaleString()}>applied {ago(a.created_at)}</span>
          </p>
          <Progress step={p.step ?? 0} state={p.state} outcome={p.outcome} />
          {p.detail && (
            <p className={`mt-2 text-xs ${stopped && p.outcome === "not_qualified" ? "text-ember/90" : "text-mute"}`}>
              {stopped && <span className="font-medium">Why: </span>}
              {p.detail}
            </p>
          )}
        </div>
        <Icon name="arrow" size={14} className={`mt-1 shrink-0 text-dim transition ${open ? "rotate-90" : ""}`} />
      </button>
      {open && <ApplicantDetail a={a} />}
    </li>
  );
}

// Six segments, one per pipeline step. Done steps are lime; the current one
// is violet (Combox working), amber (waiting) or ember/grey where it stopped.
function Progress({ step, state, outcome }) {
  const cur = state === "done" ? "bg-lime" : state === "working" ? "bg-violet" : state === "waiting" ? "bg-amber" : outcome === "not_qualified" ? "bg-ember" : "bg-white/30";
  return (
    <div className="@container mt-3" aria-label={`Step ${step + 1} of ${STEP_LABELS.length}: ${STEP_LABELS[step]}`}>
      <div className="flex gap-1">
        {STEP_LABELS.map((l, i) => (
          <span
            key={l}
            title={l}
            className={`h-1.5 flex-1 rounded-full ${i < step ? (state === "stopped" ? "bg-lime/40" : "bg-lime") : i === step ? `${cur} ${state === "working" ? "animate-pulse" : ""}` : "bg-white/8"}`}
          />
        ))}
      </div>
      <div className="mt-1 hidden gap-1 @sm:flex" aria-hidden="true">
        {STEP_LABELS.map((l, i) => (
          <span key={l} className={`flex-1 truncate text-[10px] ${i === step ? "text-fog" : "text-dim"}`}>{l}</span>
        ))}
      </div>
    </div>
  );
}

const DOT = { good: "bg-lime", bad: "bg-ember", warn: "bg-amber", info: "bg-sky" };
const MATCH = {
  listed: ["bg-lime/15 text-lime ring-lime/30", "check", "listed"],
  related: ["bg-amber/15 text-amber ring-amber/30", "minus", "related skill"],
  mentioned: ["bg-amber/15 text-amber ring-amber/30", "minus", "in cover letter"],
  missing: ["bg-ember/10 text-ember/90 ring-ember/25 line-through decoration-ember/50", "x", "missing"],
};

// The full log, skill match and interview scores for one application.
function ApplicantDetail({ a }) {
  const { session, base, notify } = useApp();
  const d = useLoad(`/admin/applications/${a.id}`, { poll: 15000 });
  const [busy, setBusy] = useState(false);
  const getCv = async () => {
    setBusy(true);
    try {
      await download(session, `${base}/admin/applications/${a.id}/cv`, a.cv.filename);
    } catch (e) {
      notify(e.message, "error");
    }
    setBusy(false);
  };
  const v = d.data;
  const p = v?.progress || a.progress || {};
  const scores = v?.interview?.scores && Object.entries(v.interview.scores).filter(([, n]) => typeof n === "number");

  return (
    <div className="grid gap-5 border-t border-white/5 p-4 pt-4 md:grid-cols-[minmax(0,1fr)_minmax(0,16rem)]">
      <div className="min-w-0">
        <Eyebrow>Progress log</Eyebrow>
        <ErrorNote error={d.error} />
        {d.loading && <div className="mt-3"><Loading rows={2} /></div>}
        {v && (
          <ol className="relative mt-3 space-y-3">
            <span className="absolute bottom-1 left-[5px] top-1 w-px bg-white/10" aria-hidden="true" />
            {v.log.map((e, i) => (
              <li key={i} className="relative flex gap-3">
                <span className={`relative z-10 mt-1 shrink-0 rounded-full ring-4 ring-panel size-[11px] ${DOT[e.tone] || DOT.info}`} />
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-baseline gap-x-2">
                    <p className="text-sm">{e.title}</p>
                    <time className="text-[11px] text-dim" dateTime={e.at} title={when(e.at)}>{ago(e.at)}</time>
                  </div>
                  {e.detail && <p className="mt-0.5 text-xs text-mute">{e.detail}</p>}
                </div>
              </li>
            ))}
            {p.state !== "done" && (
              <li className="relative flex gap-3">
                <span
                  className={`relative z-10 mt-1 shrink-0 rounded-full ring-4 ring-panel size-[11px] ${
                    p.state === "stopped" ? (p.outcome === "not_qualified" ? "bg-ember" : "bg-white/40") : p.state === "working" ? "pulse-dot bg-violet text-violet" : "bg-amber"
                  }`}
                />
                <div className="min-w-0 flex-1">
                  <p className="text-sm font-medium">{p.state === "stopped" ? "Final" : "Now"}: {p.headline}</p>
                  {p.detail && <p className="mt-0.5 text-xs text-mute">{p.detail}</p>}
                </div>
              </li>
            )}
          </ol>
        )}
      </div>

      <div className="min-w-0 space-y-5">
        {v?.skill_match?.length > 0 && (
          <div>
            <Eyebrow>Skill match {a.shortlist_score != null && <span className="text-fog">· {Math.round(a.shortlist_score * 100)}%</span>}</Eyebrow>
            <ul className="mt-2 flex flex-wrap gap-1.5">
              {v.skill_match.map(({ skill, match }) => {
                const [cls, icon, why] = MATCH[match] || MATCH.missing;
                return (
                  <li key={skill} title={why} className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] ring-1 ring-inset ${cls}`}>
                    <Icon name={icon} size={11} /> {human(skill)}
                  </li>
                );
              })}
            </ul>
            {a.skills?.length > 0 && <p className="mt-2 text-[11px] text-dim">They listed: {a.skills.map(human).join(", ")}</p>}
          </div>
        )}

        {scores?.length > 0 && (
          <div>
            <Eyebrow>
              Interview <span className={a.interview_score >= v.interview.pass_mark ? "text-lime" : "text-ember"}>· {Number(a.interview_score ?? 0).toFixed(2)}/5</span>
            </Eyebrow>
            <ul className="mt-2 space-y-2">
              {scores.map(([k, n]) => (
                <li key={k}>
                  <div className="flex justify-between text-[11px]">
                    <span className="text-mute">{human(k)}</span>
                    <span className="num text-fog">{n}/5</span>
                  </div>
                  <div className="relative mt-1 h-1.5 rounded-full bg-white/8">
                    <span className={`absolute inset-y-0 left-0 rounded-full ${n >= v.interview.pass_mark ? "bg-lime" : "bg-amber"}`} style={{ width: `${(n / 5) * 100}%` }} />
                    <span className="absolute -inset-y-0.5 w-px bg-fog/50" style={{ left: `${(v.interview.pass_mark / 5) * 100}%` }} title={`Pass mark ${v.interview.pass_mark}`} />
                  </div>
                </li>
              ))}
            </ul>
            <p className="mt-2 text-[11px] text-dim">The average needs {v.interview.pass_mark} or more for a contract.</p>
          </div>
        )}

        <div>
          <Eyebrow>Contact</Eyebrow>
          <a href={`mailto:${a.email}`} className="mt-1.5 block truncate text-sm text-fog underline-offset-2 hover:underline">{a.email}</a>
          <div className="mt-2">
            {a.cv ? (
              <Button size="sm" variant="ghost" disabled={busy} onClick={getCv} title={a.cv.filename}>
                <Icon name="download" size={14} /> {busy ? "Downloading…" : `CV · ${a.cv.size_bytes < 1048576 ? `${Math.max(1, Math.round(a.cv.size_bytes / 1024))} KB` : `${(a.cv.size_bytes / 1048576).toFixed(1)} MB`}`}
              </Button>
            ) : (
              <span className="text-xs text-dim">No CV attached</span>
            )}
          </div>
        </div>

        {a.untrusted_message && (
          <div>
            <Eyebrow>Their note</Eyebrow>
            <p className="mt-1.5 whitespace-pre-wrap break-words text-xs text-mute" title="Written by the applicant">{a.untrusted_message}</p>
          </div>
        )}
      </div>
    </div>
  );
}
