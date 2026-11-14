// Video meetings and AI interviews: create a room, dispatch the AI
// interviewer, and read consent, scores, summary, and transcript.
import { useEffect, useState } from "react";
import DateTimePicker from "../DateTimePicker.jsx";
import {
  Badge, Button, Empty, ErrorNote, Eyebrow, Icon, Input, Loading, Modal, PageHead, Panel, STATUS_TONE, Textarea,
  human, useApp, useCompanyApi, useLoad, when,
} from "../ui.jsx";

const DEFAULT_INSTRUCTIONS = "Introduce yourself as an AI. Ask for recording consent before any question. Do not discuss pay.";
const safeHref = (u) => (/^https?:\/\//i.test(u || "") ? u : undefined);

export default function Meetings() {
  const { data, error, loading, reload } = useLoad("/admin/meetings");
  const [open, setOpen] = useState(null);
  const [wizard, setWizard] = useState(null);
  // Join links are only returned once, at creation (the server keeps a hash).
  // Keep them for this tab so they stay reachable from the interview page.
  const [joinLinks, setJoinLinks] = useState({});
  // Bumped after create/dispatch so the open detail panel reloads too.
  const [version, setVersion] = useState(0);
  const list = data?.meetings || [];
  useEffect(() => {
    if (!open && list.length) setOpen(list[0].id);
  }, [list, open]);

  return (
    <>
      <PageHead kicker="AI interviewer · consent first" title="Interviews">
        <Button onClick={() => setWizard({ step: 1 })}>
          <Icon name="plus" size={16} /> New interview
        </Button>
      </PageHead>
      <p className="reveal -mt-2 mb-6 max-w-2xl text-sm text-mute">AI interviews and meetings. The interviewer always says it's an AI and asks consent before recording.</p>
      <ErrorNote error={error} />
      {loading && <Loading rows={4} />}
      {data && list.length === 0 && <Empty title="No interviews yet" text="Interviews appear here when ForeMan shortlists candidates. You can also create a room yourself." />}
      {list.length > 0 && (
        <div className="grid gap-4 xl:grid-cols-[minmax(0,22rem)_minmax(0,1fr)]">
          <div className="scrollbar-none -mx-4 flex gap-2 overflow-x-auto px-4 pb-1 sm:-mx-8 sm:px-8 xl:mx-0 xl:block xl:space-y-2 xl:overflow-visible xl:px-0">
            {list.map((m, i) => {
              const d = new Date(m.scheduled_at);
              const on = m.id === open;
              return (
                <button
                  key={m.id}
                  onClick={() => setOpen(m.id)}
                  aria-current={on ? "true" : undefined}
                  style={{ "--i": i }}
                  className={`reveal flex w-72 shrink-0 items-center gap-3 rounded-2xl p-3 text-left ring-1 ring-inset transition xl:w-full ${on ? "bg-white/6 ring-lime/40" : "bg-white/2 ring-white/5 hover:bg-white/4"}`}
                >
                  <span className={`flex shrink-0 flex-col items-center justify-center rounded-xl leading-none size-14 ${on ? "bg-lime text-ink" : "bg-white/5"}`}>
                    <span className="font-mono text-[9px] uppercase opacity-70">{d.toLocaleString(undefined, { month: "short" })}</span>
                    <span className="font-display text-xl font-semibold">{d.getDate()}</span>
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium">{m.title}</span>
                    <span className="block truncate text-xs text-mute">
                      {d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })} · {m.invitees.join(", ")}
                    </span>
                    <span className="mt-1 block"><Badge tone={STATUS_TONE[m.status]} dot={m.status === "dispatched"}>{human(m.status)}</Badge></span>
                  </span>
                </button>
              );
            })}
          </div>
          {open ? (
            <Detail key={`${open}-${version}`} id={open} links={joinLinks[open]} onDispatch={(m) => setWizard({ step: 2, meetingId: m.id, title: m.title, instructions: m.instructions, questions: m.questions })} />
          ) : (
            <div />
          )}
        </div>
      )}
      <Wizard
        state={wizard}
        setState={setWizard}
        onDone={() => {
          reload();
          setVersion((v) => v + 1);
        }}
        onCreated={(id, links) => {
          setJoinLinks((m) => ({ ...m, [id]: links }));
          setOpen(id);
        }}
        links={wizard?.meetingId ? joinLinks[wizard.meetingId] || [] : []}
      />
    </>
  );
}

function Detail({ id, links, onDispatch }) {
  // Polls so consent, transcript and completion appear while you watch.
  const { data, error, reload } = useLoad(`/admin/meetings/${id}`, { poll: 15000 });
  const call = useCompanyApi();
  const { notify } = useApp();
  const [mine, setMine] = useState(null);
  const [joining, setJoining] = useState(false);
  const m = data?.meeting;
  // A fresh link for you: invite links are shown only once, at creation.
  const join = async () => {
    setJoining(true);
    try {
      const r = await call(`/admin/meetings/${id}/join-link`, { method: "POST" });
      setMine(r.join_url);
      window.open(r.join_url, "_blank", "noopener,noreferrer");
    } catch (e) {
      notify(e.message, "error");
    }
    setJoining(false);
  };
  if (error && !m) return <ErrorNote error={error} />;
  if (!m) return <Panel className="p-6"><Loading rows={6} /></Panel>;
  const scores = m.scores ? Object.entries(m.scores) : [];
  const avg = scores.length ? scores.reduce((s, [, v]) => s + Number(v), 0) / scores.length : null;
  const consent = m.consent ? (m.consent.granted ? "Granted" : "Declined") : "Not yet";
  return (
    <Panel className="min-w-0 p-6 sm:p-8">
      <div className="flex flex-wrap items-start gap-3">
        <div className="mr-auto min-w-0">
          <Eyebrow>{when(m.scheduled_at)}{m.room ? ` · room ${m.room}` : ""}</Eyebrow>
          <h2 className="mt-2 font-display text-2xl font-semibold tracking-tight">{m.title}</h2>
        </div>
        <div className="flex items-center gap-2">
          <Badge tone={STATUS_TONE[m.status]} dot={m.status === "dispatched"}>{human(m.status)}</Badge>
          {["scheduled", "dispatched"].includes(m.status) && (
            <Button size="sm" onClick={join} disabled={joining}>
              <Icon name="external" size={14} /> {joining ? "Opening…" : "Join room"}
            </Button>
          )}
        </div>
      </div>
      {mine && (
        <p className="mt-3 text-sm text-mute">
          Your link (valid for a day):{" "}
          <a href={safeHref(mine)} target="_blank" rel="noreferrer noopener" className="break-all font-mono text-xs text-lime underline decoration-lime/40 underline-offset-4 hover:decoration-lime">
            {mine}
          </a>
        </p>
      )}

      <div className="mt-6 grid grid-cols-[repeat(auto-fit,minmax(11rem,1fr))] gap-3">
        <Fact label="Consent" value={consent} tone={m.consent ? (m.consent.granted ? "text-lime" : "text-ember") : "text-mute"} sub={m.consent?.statement && `“${m.consent.statement}”`} />
        <Fact label="Average score" value={avg == null ? "—" : `${avg.toFixed(1)} / 5`} tone="text-fog" />
        <Fact label="Invitees" value={m.invitees.length} tone="text-fog" sub={m.invitees.join(", ")} />
      </div>

      {links?.length > 0 ? (
        <section className="mt-6">
          <Eyebrow className="mb-2">Join links</Eyebrow>
          <JoinLinks links={links} />
        </section>
      ) : (
        ["scheduled", "dispatched"].includes(m.status) && (
          <p className="mt-6 rounded-2xl bg-white/3 px-4 py-3 text-xs text-mute ring-1 ring-white/5">
            Invitees' links are shown once, when the room is created (the server keeps only a fingerprint). Use Join room to open it yourself.
          </p>
        )
      )}

      {m.status === "scheduled" && (
        <div className="mt-6 flex flex-wrap items-center gap-3 rounded-2xl bg-violet/10 p-4 ring-1 ring-violet/25">
          <span className="text-sm">The room is ready. Dispatch the AI interviewer with instructions and questions.</span>
          <Button size="sm" className="ml-auto" onClick={() => onDispatch(m)}>Dispatch agent</Button>
        </div>
      )}

      {m.status === "dispatched" && !m.dispatched_at && (
        <div className="mt-6 flex flex-wrap items-center gap-3 rounded-2xl bg-violet/10 p-4 ring-1 ring-violet/25">
          <span className="text-sm">The AI interviewer joins when the applicant does, with {m.questions?.length || 0} question{m.questions?.length === 1 ? "" : "s"}. You can change them until then.</span>
          <Button size="sm" variant="ghost" className="ml-auto" onClick={() => onDispatch(m)}>Edit questions</Button>
        </div>
      )}

      {scores.length > 0 && (
        <section className="mt-8">
          <Eyebrow className="mb-3">Scores</Eyebrow>
          <div className="space-y-3">
            {scores.map(([k, v], i) => (
              <div key={k} className="grid grid-cols-[minmax(0,8rem)_1fr_3rem] items-center gap-3 text-sm sm:grid-cols-[minmax(0,10rem)_1fr_3rem]">
                <span className="truncate text-mute" title={human(k)}>{human(k)}</span>
                <div className="flex h-2.5 gap-1" role="img" aria-label={`${human(k)}: ${v} of 5`}>
                  {[1, 2, 3, 4, 5].map((n) => (
                    <span key={n} className={`grow-x flex-1 rounded-full ${n <= v ? (v >= 4 ? "bg-lime" : v >= 3 ? "bg-amber" : "bg-ember") : "bg-white/5"}`} style={{ "--i": i + n }} />
                  ))}
                </div>
                <span className="num text-right">{v} / 5</span>
              </div>
            ))}
          </div>
        </section>
      )}

      {m.summary && (
        <section className="mt-8">
          <Eyebrow className="mb-2">Summary</Eyebrow>
          <p className="whitespace-pre-wrap text-sm/relaxed text-fog/90">{m.summary}</p>
        </section>
      )}

      {m.transcript?.length > 0 && (
        <section className="mt-8">
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <Eyebrow>Transcript</Eyebrow>
            <Badge tone="amber">untrusted participant text</Badge>
          </div>
          <div className="max-h-112 space-y-2 overflow-y-auto rounded-2xl bg-black/30 p-4 ring-1 ring-white/5">
            {m.transcript.map((t, i) => {
              const agent = t.speaker === "agent";
              return (
                <div key={i} className={`flex ${agent ? "justify-start" : "justify-end"}`}>
                  <div className={`max-w-[85%] rounded-2xl px-4 py-2.5 text-sm ${agent ? "rounded-bl-md bg-violet/15 ring-1 ring-violet/25" : "rounded-br-md bg-white/6"}`}>
                    <p className={`mb-0.5 font-mono text-[10px] uppercase tracking-wider ${agent ? "text-violet" : "text-mute"}`}>{agent ? "AI interviewer" : human(t.speaker)}</p>
                    {t.text}
                  </div>
                </div>
              );
            })}
          </div>
        </section>
      )}

      <AddRecipient id={m.id} recipients={m.extra_recipients || []} onAdded={reload} />
    </Panel>
  );
}

// One row per invitee: who it's for, the link, copy and open.
function JoinLinks({ links }) {
  const { notify } = useApp();
  const copy = (url) => navigator.clipboard?.writeText(url).then(() => notify("Join link copied"), () => notify("Copy failed", "error"));
  return (
    <ul className="space-y-2">
      {links.map((l, i) => (
        <li key={`${l.email}-${i}`} className="flex flex-wrap items-center gap-2 rounded-xl bg-black/40 p-2 pl-3 ring-1 ring-white/10 sm:flex-nowrap">
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm text-fog">{l.email}</span>
            <a
              href={safeHref(l.join_url)}
              target="_blank"
              rel="noreferrer noopener"
              title={l.join_url}
              className="block truncate font-mono text-[11px] text-dim underline decoration-white/20 underline-offset-4 transition hover:text-lime hover:decoration-lime"
            >
              {l.join_url}
            </a>
          </span>
          <span className="flex shrink-0 gap-1.5">
            <Button type="button" size="sm" variant="ghost" onClick={() => copy(l.join_url)} aria-label={`Copy join link for ${l.email}`}>
              <Icon name="copy" size={13} /> Copy
            </Button>
            <a
              href={safeHref(l.join_url)}
              target="_blank"
              rel="noreferrer noopener"
              aria-label={`Open join link for ${l.email}`}
              className="inline-flex items-center gap-1.5 rounded-full bg-lime px-3 py-1.5 text-xs font-medium text-ink transition hover:bg-[#e4ff7a]"
            >
              Open <Icon name="external" size={13} />
            </a>
          </span>
        </li>
      ))}
    </ul>
  );
}

function Fact({ label, value, sub, tone }) {
  return (
    <div className="min-w-0 rounded-2xl bg-white/3 p-4 ring-1 ring-white/5">
      <p className="font-mono text-[10px] uppercase tracking-widest text-dim">{label}</p>
      <p className={`mt-1 font-display text-lg font-semibold ${tone}`}>{value}</p>
      {sub && <p className="mt-1 truncate text-xs text-mute" title={sub}>{sub}</p>}
    </div>
  );
}

// Who else gets the summary when the interview completes.
function AddRecipient({ id, recipients, onAdded }) {
  const call = useCompanyApi();
  const { notify } = useApp();
  const [email, setEmail] = useState("");
  const [busy, setBusy] = useState(false);
  const add = async (e) => {
    e.preventDefault();
    setBusy(true);
    try {
      await call(`/admin/meetings/${id}/recipients`, { method: "POST", body: { email } });
      notify(`${email} will get the summary.`);
      setEmail("");
      onAdded();
    } catch (err) {
      notify(err.message, "error");
    }
    setBusy(false);
  };
  return (
    <section className="mt-8 border-t border-white/5 pt-6">
      <Eyebrow className="mb-3">Summary recipients</Eyebrow>
      {recipients.length > 0 && (
        <ul className="mb-4 flex flex-wrap gap-1.5" aria-label="Also receiving the summary">
          {recipients.map((r) => (
            <li key={r}><Badge tone="sky">{r}</Badge></li>
          ))}
        </ul>
      )}
      <form onSubmit={add} className="flex flex-wrap items-end gap-3">
        <Input label="Send the summary to someone else" type="email" value={email} onChange={setEmail} placeholder="name@company.com" className="min-w-0 flex-1 basis-56" />
        <Button type="submit" variant="ghost" disabled={busy || !email.includes("@")}>Add</Button>
      </form>
    </section>
  );
}

function Wizard({ state, setState, onDone, onCreated, links }) {
  const call = useCompanyApi();
  const { notify } = useApp();
  const [title, setTitle] = useState("");
  const [at, setAt] = useState("");
  const [invitees, setInvitees] = useState("");
  const [instructions, setInstructions] = useState(DEFAULT_INSTRUCTIONS);
  const [questions, setQuestions] = useState(["", ""]);
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(false);
  // The wizard stays mounted while closed (state is null), so "Edit
  // questions" loads the interview's current questions when it opens.
  const editing = state?.questions?.length ? state.meetingId : null;
  useEffect(() => {
    if (!editing) return;
    setInstructions(state.instructions || DEFAULT_INSTRUCTIONS);
    setQuestions(state.questions);
  }, [editing]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!state) return null;
  // Each new interview starts from a clean form.
  const close = () => {
    setState(null);
    setError(null);
    setTitle("");
    setAt("");
    setInvitees("");
    setInstructions(DEFAULT_INSTRUCTIONS);
    setQuestions(["", ""]);
  };

  const create = async (e) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const r = await call("/admin/meetings", {
        method: "POST",
        body: { title, scheduled_at: new Date(at).toISOString(), invitees: [...new Set(invitees.split(",").map((s) => s.trim()).filter(Boolean))] },
      });
      onCreated(r.meeting_id, r.join_links || []);
      setState({ step: 2, meetingId: r.meeting_id, title, created: true, emailed: r.invites_emailed });
      onDone();
    } catch (err) {
      setError(err.message);
    }
    setBusy(false);
  };
  const dispatch = async (e) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await call(`/admin/meetings/${state.meetingId}/dispatch`, { method: "POST", body: { instructions, questions: questions.map((q) => q.trim()).filter(Boolean) } });
      notify("Interviewer dispatched");
      onDone();
      close();
    } catch (err) {
      setError(err.message);
    }
    setBusy(false);
  };
  // invites_emailed is a count, or the reason sending failed.
  const notSent = typeof state.emailed === "string";

  return (
    <Modal open onClose={close} wide backdropClose={false} kicker={`Step ${state.step} of 2`} title={state.step === 1 ? "Create the room" : "Brief the interviewer"}>
      <div className="mb-6 flex gap-2" aria-hidden="true">
        {[1, 2].map((s) => <span key={s} className={`h-1 flex-1 rounded-full ${s <= state.step ? "bg-lime" : "bg-white/10"}`} />)}
      </div>
      {state.step === 1 ? (
        <form onSubmit={create} className="space-y-4">
          <Input label="Title" value={title} onChange={setTitle} placeholder="Interview: Brand designer" maxLength={200} required autoFocus />
          <DateTimePicker label="When" value={at} onChange={setAt} />
          <Input label="Invitees" value={invitees} onChange={setInvitees} placeholder="a@x.com, b@y.com" hint="Comma-separated emails. Each gets a personal join link." required />
          <ErrorNote error={error} />
          <div className="grid grid-cols-2 gap-3 pt-2">
            <Button type="button" variant="ghost" onClick={close}>Cancel</Button>
            <Button type="submit" disabled={busy || !title.trim() || !at || !invitees.trim()}>{busy ? "Creating…" : "Create room"}</Button>
          </div>
        </form>
      ) : (
        <form onSubmit={dispatch} className="space-y-4">
          {state.created && (
            <div className={`rounded-2xl p-4 ring-1 ${notSent ? "bg-amber/6 ring-amber/25" : "bg-lime/6 ring-lime/25"}`}>
              <p className={`flex items-center gap-2 text-sm font-semibold ${notSent ? "text-amber" : "text-lime"}`}>
                <Icon name="check" size={16} /> Room created
              </p>
              <p className="mt-1 mb-3 text-xs text-mute">
                {notSent
                  ? `The invites weren't emailed (${state.emailed}). Send each invitee their link yourself.`
                  : "Invites with these personal join links have been emailed."}{" "}
                Links are shown once (the server keeps only a fingerprint). They stay on this interview's page until you close this tab.
              </p>
              {links.length > 0 && <JoinLinks links={links} />}
            </div>
          )}
          <div className="flex flex-wrap items-baseline gap-2">
            <p className="mr-auto text-sm text-mute">AI interviewer for <span className="text-fog">{state.title}</span></p>
            {state.created && <span className="text-xs text-dim">Default questions are already set; replace them here or later from the interview page.</span>}
          </div>
          <Textarea label="Instructions" value={instructions} onChange={setInstructions} rows={3} maxLength={4000} />
          <div>
            <span className="text-sm text-mute" id="questions-label">Questions</span>
            <ol className="mt-1.5 space-y-2" aria-labelledby="questions-label">
              {questions.map((q, i) => (
                <li key={i} className="flex items-center gap-2">
                  <span className="w-6 shrink-0 text-right font-mono text-xs text-dim">{i + 1}</span>
                  <input
                    value={q}
                    onChange={(e) => setQuestions(questions.map((x, j) => (j === i ? e.target.value : x)))}
                    placeholder="Ask about…"
                    aria-label={`Question ${i + 1}`}
                    maxLength={500}
                    className="w-full rounded-xl border border-white/10 bg-black/40 px-3.5 py-2.5 text-sm text-fog placeholder:text-dim focus:border-lime/60 focus:outline-none focus:ring-4 focus:ring-lime/10"
                  />
                  <button type="button" onClick={() => setQuestions(questions.filter((_, j) => j !== i))} className="rounded-full p-2 text-dim hover:text-ember" aria-label={`Remove question ${i + 1}`}>
                    <Icon name="x" size={14} />
                  </button>
                </li>
              ))}
            </ol>
            {questions.length < 20 && (
              <Button type="button" variant="link" size="sm" className="mt-2 ml-8" onClick={() => setQuestions([...questions, ""])}>+ Add question</Button>
            )}
          </div>
          <ErrorNote error={error} />
          <div className="grid grid-cols-2 gap-3 pt-2">
            <Button type="button" variant="ghost" onClick={close}>{state.created ? "Done" : "Cancel"}</Button>
            <Button type="submit" disabled={busy || !questions.some((q) => q.trim())}>{busy ? "Dispatching…" : "Dispatch"}</Button>
          </div>
          <p className="text-center text-xs text-dim">Policy-checked. The agent always discloses it is an AI and asks for consent first.</p>
        </form>
      )}
    </Modal>
  );
}
