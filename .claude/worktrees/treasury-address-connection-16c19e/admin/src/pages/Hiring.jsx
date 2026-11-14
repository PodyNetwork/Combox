// Hiring: the company's public jobs board and how candidates move through
// shortlisting, the AI interview, and the contract.
import { useState } from "react";
import { download } from "../api.js";
import {
  Badge, Button, Chip, CopyField, CountUp, Empty, ErrorNote, Eyebrow, Icon, Loading, PageHead, Panel, PanelHead,
  ago, day, human, useApp, useLoad, useSiteUrl, usd,
} from "../ui.jsx";

const STEPS = [
  ["Shortlist", "Applicants are shortlisted by skill match. What they write is stored as data, never acted on."],
  ["Interview", "The top three get an email invite to a video interview with your AI interviewer, which asks consent first."],
  ["Contract", "The best candidate at or above the bar gets a contract from your approved DocuSeal template."],
  ["Onboard", "Once signed, they get their task briefs. Before the first payout they verify a NEAR address and submit a W-9 or W-8BEN."],
];

export default function Hiring() {
  const { platform: isPlatform, navigate } = useApp();
  const jobs = useLoad("/jobs");
  const boardUrl = useSiteUrl()("/jobs");
  const list = jobs.data?.jobs || [];

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
      <p className="reveal -mt-2 mb-6 max-w-2xl text-sm text-mute">When no one on the team can do a task, Combox posts a job on your board and runs the hiring for you.</p>

      <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_24rem]">
        <div className="min-w-0 space-y-4">
        <Panel className="p-6">
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
              {list.map((j, i) => (
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
                    <div className="mt-3 flex flex-wrap gap-1">{j.skills.map((s) => <Chip key={s}>{human(s)}</Chip>)}</div>
                  )}
                  {j.kpis?.length > 0 && (
                    <ul className="mt-3 space-y-1 border-t border-white/5 pt-3 text-xs text-mute" aria-label="KPIs">
                      {j.kpis.map((k) => (
                        <li key={k} className="flex items-center gap-2">
                          <span className="rounded-full bg-lime/70 size-1" aria-hidden="true" />
                          {human(k)}
                        </li>
                      ))}
                    </ul>
                  )}
                </li>
              ))}
            </ul>
          )}
        </Panel>
        <Applicants />
        </div>

        <div className="space-y-4">
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

const STATUS_TONE = { applied: undefined, shortlisted: "sky", invited: "violet", interviewed: "violet", contract_sent: "amber", hired: "lime", rejected: undefined, declined: undefined };

// Everyone who applied on the jobs board, newest first, with their CV.
// Names and notes are what applicants typed: shown as plain text only.
function Applicants() {
  const { session, base, notify } = useApp();
  const apps = useLoad("/admin/applications", { poll: 30000 });
  const [busy, setBusy] = useState(null);
  const list = apps.data?.applications || [];
  const getCv = async (a) => {
    setBusy(a.id);
    try {
      await download(session, `${base}/admin/applications/${a.id}/cv`, a.cv.filename);
    } catch (e) {
      notify(e.message, "error");
    }
    setBusy(null);
  };
  return (
    <Panel i={1} className="p-6">
      <PanelHead
        eyebrow="From your jobs board"
        title="Applicants"
        action={apps.data && <span className="num font-display text-4xl font-semibold leading-none text-sky"><CountUp value={list.length} /></span>}
      />
      <ErrorNote error={apps.error} />
      {apps.loading && <Loading rows={3} />}
      {apps.data && list.length === 0 && <Empty title="No applicants yet" text="Applications and CVs appear here as people apply on your board." />}
      {list.length > 0 && (
        <ul className="divide-y divide-white/5">
          {list.map((a) => (
            <li key={a.id} className="flex flex-wrap items-start gap-x-4 gap-y-2 py-3.5 first:pt-0 last:pb-0">
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-2">
                  <p className="truncate font-medium">{a.name}</p>
                  <Badge tone={STATUS_TONE[a.status]}>{human(a.status)}</Badge>
                  {a.interview_score != null && <span className="num text-xs text-mute">interview {Number(a.interview_score).toFixed(1)}</span>}
                </div>
                <p className="mt-0.5 truncate text-xs text-dim">
                  {a.job_title} · <span title={new Date(a.created_at).toLocaleString()}>{ago(a.created_at)}</span> · {a.email}
                </p>
                {a.untrusted_message && <p className="mt-1.5 line-clamp-2 text-sm text-mute" title="Written by the applicant">{a.untrusted_message}</p>}
              </div>
              {a.cv ? (
                <Button size="sm" variant="ghost" className="shrink-0" disabled={busy === a.id} onClick={() => getCv(a)} title={`${a.cv.filename} · ${a.cv.size_bytes < 1048576 ? `${Math.max(1, Math.round(a.cv.size_bytes / 1024))} KB` : `${(a.cv.size_bytes / 1048576).toFixed(1)} MB`}`}>
                  <Icon name="download" size={14} /> {busy === a.id ? "Downloading…" : "CV"}
                </Button>
              ) : (
                <span className="shrink-0 text-xs text-dim">No CV</span>
              )}
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}
