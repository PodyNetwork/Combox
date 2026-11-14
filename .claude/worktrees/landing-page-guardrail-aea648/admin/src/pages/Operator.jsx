// Platform operator: every company, its subscription and payments, and
// suspending or resuming one. Only for emails in AXOM_PLATFORM_OPERATORS.
import { useCallback, useEffect, useState } from "react";
import { platform } from "../api.js";
import { Badge, Button, CountUp, Empty, ErrorNote, Eyebrow, Icon, Loading, Modal, PageHead, Panel, PanelHead, Textarea, day, pctOf, useApp, usd } from "../ui.jsx";

const subLabel = (s) => (s.state === "trial" ? `Trial to ${day(s.ends_at)}` : s.state === "active" ? `Paid to ${day(s.paid_until)}` : `Ended ${day(s.since)}`);
const SUB_TONE = { trial: "sky", active: "lime", expired: "amber" };
// Company rows: name, subscription, owner, members, paid, action.
const COLS = "xl:grid-cols-[minmax(0,2.2fr)_minmax(0,1.5fr)_minmax(0,1.6fr)_4.5rem_8rem_6.5rem]";

function SubBadge({ c }) {
  return c.status === "suspended" ? <Badge tone="ember">Suspended</Badge> : <Badge tone={SUB_TONE[c.subscription.state]}>{subLabel(c.subscription)}</Badge>;
}

export default function Operator() {
  const { session, notify, newCompany, company } = useApp();
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  const [suspending, setSuspending] = useState(null);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(null);
  const reload = useCallback(
    () =>
      platform(session, "/operator/companies").then(
        (d) => {
          setData(d);
          setError(null);
        },
        (e) => setError(e.message),
      ),
    [session],
  );
  useEffect(() => {
    reload();
  }, [reload]);
  const act = async (slug, action, body) => {
    setBusy(slug);
    try {
      await platform(session, `/operator/companies/${slug}/${action}`, { method: "POST", body: body || {} });
      notify(action === "suspend" ? `${slug} suspended.` : `${slug} resumed.`);
      setSuspending(null);
      setReason("");
    } catch (e) {
      notify(e.message, "error");
    }
    setBusy(null);
    reload();
  };
  const closeSuspend = () => {
    setSuspending(null);
    setReason("");
  };
  const t = data?.totals;
  const mix = t
    ? [
        { key: "active", label: "Paying", n: t.active, dot: "bg-lime" },
        { key: "trial", label: "On trial", n: t.trial, dot: "bg-sky" },
        { key: "expired", label: "Lapsed", n: t.expired, dot: "bg-amber", hot: "text-amber" },
        { key: "suspended", label: "Suspended", n: t.suspended, dot: "bg-ember", hot: "text-ember" },
      ]
    : [];

  return (
    <>
      <PageHead kicker="Platform operator" title="All companies">
        {!company && (
          <Button variant="ghost" onClick={newCompany}>
            <Icon name="plus" size={16} /> Create your own company
          </Button>
        )}
      </PageHead>
      <p className="reveal -mt-2 mb-6 max-w-2xl text-sm text-mute">
        Every company on this ForeMan. Suspending stops a company's API, jobs board, background jobs, and payouts; its data is kept.
      </p>
      <ErrorNote error={error} />
      {!data && !error && <Loading rows={5} />}
      {data && (
        <div className="space-y-4">
          <div className="flex flex-col gap-4 xl:flex-row">
            <Panel className="flex flex-col p-6 sm:p-8 xl:shrink-0">
              <Eyebrow>Paid in total</Eyebrow>
              <p className="num mt-4 font-display text-4xl font-semibold leading-none tracking-tighter text-lime sm:text-6xl">
                <CountUp value={t.paid_usd} format={(v) => usd(v)} />
              </p>
              <p className="mt-3 text-sm text-mute">In NEAR, at each invoice's rate</p>
            </Panel>

            <Panel i={1} className="min-w-0 p-6 xl:flex-1">
              <PanelHead eyebrow="Companies" title={`${t.companies} ${t.companies === 1 ? "company" : "companies"}`} />
              <div className="flex h-2 overflow-hidden rounded-full bg-white/6" role="img" aria-label={mix.map((m) => `${m.n} ${m.label.toLowerCase()}`).join(", ")}>
                {mix.map((m, i) => (m.n > 0 ? <div key={m.key} className={`grow-x h-full ${m.dot}`} style={{ width: `${pctOf(m.n, t.companies)}%`, "--i": i }} /> : null))}
              </div>
              <dl className="mt-5 grid grid-cols-2 gap-4 sm:grid-cols-4">
                {mix.map((m) => (
                  <div key={m.key}>
                    <dt className="flex items-center gap-2 text-xs text-mute">
                      <span className={`shrink-0 rounded-full size-1.5 ${m.dot}`} />
                      {m.label}
                    </dt>
                    <dd className={`num mt-1 font-display text-3xl font-semibold leading-none ${m.n && m.hot ? m.hot : m.n ? "text-fog" : "text-dim"}`}>
                      <CountUp value={m.n} />
                    </dd>
                  </div>
                ))}
              </dl>
            </Panel>
          </div>

          <Panel i={2} className="p-2 sm:p-3">
            {data.companies.length === 0 ? (
              <div className="p-3">
                <Empty title="No companies yet" text="Companies appear here once someone signs up and creates one." />
              </div>
            ) : (
              <>
                <div className={`hidden gap-4 px-4 pb-2 pt-3 font-mono text-[10px] uppercase tracking-widest text-dim xl:grid ${COLS}`} aria-hidden="true">
                  <span>Company</span>
                  <span>Subscription</span>
                  <span>Owner</span>
                  <span>Members</span>
                  <span>Paid</span>
                  <span />
                </div>
                <ul className="divide-y divide-white/7">
                  {data.companies.map((c) => {
                    const suspended = c.status === "suspended";
                    return (
                      <li key={c.slug} className={`grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-2 rounded-2xl px-4 py-4 transition hover:bg-white/3 ${COLS} ${suspended ? "bg-ember/4" : ""}`}>
                        <div className="min-w-0">
                          <p className="truncate text-sm font-medium" title={c.name}>{c.name}</p>
                          <p className="mt-0.5 text-xs text-mute">
                            <span className="font-mono">/t/{c.slug}</span> · since {day(c.created_at)}
                            {c.has_treasury ? " · treasury" : ""}
                          </p>
                        </div>
                        <div className="hidden xl:block">
                          <SubBadge c={c} />
                        </div>
                        <p className="hidden truncate text-sm text-mute xl:block" title={c.owner_email}>
                          <span className="sr-only">Owner: </span>
                          {c.owner_email}
                        </p>
                        <p className="num hidden text-sm xl:block">
                          <span className="sr-only">Members: </span>
                          {c.members}
                        </p>
                        <p className="hidden text-sm xl:block">
                          <span className="sr-only">Paid: </span>
                          <span className="num">{usd(c.paid_usd)}</span> <span className="text-xs text-dim">({c.paid_invoices})</span>
                        </p>
                        <div className="justify-self-end">
                          {suspended ? (
                            <Button size="sm" onClick={() => act(c.slug, "resume")} disabled={busy === c.slug} aria-label={`Resume ${c.name}`}>
                              {busy === c.slug ? "Resuming…" : "Resume"}
                            </Button>
                          ) : (
                            <Button size="sm" variant="ghost" className="text-ember!" onClick={() => setSuspending(c)} disabled={busy === c.slug} aria-label={`Suspend ${c.name}`}>
                              Suspend
                            </Button>
                          )}
                        </div>
                        {/* Small screens: the other columns as one line. */}
                        <div className="col-span-2 flex flex-wrap items-center gap-x-3 gap-y-1.5 text-xs text-mute xl:hidden">
                          <SubBadge c={c} />
                          <span className="min-w-0 truncate">{c.owner_email}</span>
                          <span>
                            <span className="num text-fog">{c.members}</span> {c.members === 1 ? "member" : "members"}
                          </span>
                          <span>
                            <span className="num text-fog">{usd(c.paid_usd)}</span> paid ({c.paid_invoices})
                          </span>
                        </div>
                      </li>
                    );
                  })}
                </ul>
              </>
            )}
          </Panel>
        </div>
      )}

      <Modal open={!!suspending} onClose={closeSuspend} kicker="Are you sure?" title={`Suspend ${suspending?.name ?? ""}?`}>
        <div className="hazard mb-5 h-2 rounded-full" />
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (reason.trim()) act(suspending.slug, "suspend", { reason });
          }}
          className="space-y-4"
        >
          <p className="text-sm text-mute">Its API, jobs board, background jobs, and payouts stop until you resume it. Members can still see billing.</p>
          <Textarea label="Reason (recorded in the audit log)" value={reason} onChange={setReason} rows={2} required autoFocus />
          <div className="grid grid-cols-2 gap-3 pt-2">
            <Button type="button" variant="ghost" onClick={closeSuspend}>Cancel</Button>
            <Button type="submit" variant="danger" disabled={!reason.trim() || busy === suspending?.slug}>
              {busy === suspending?.slug ? "Suspending…" : "Suspend"}
            </Button>
          </div>
        </form>
      </Modal>
    </>
  );
}
