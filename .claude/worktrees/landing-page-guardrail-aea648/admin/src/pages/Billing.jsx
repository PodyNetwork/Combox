// Subscription, paid in NEAR: open an invoice at the current rate, send the
// NEAR from any wallet, then submit the transaction so ForeMan can check it on
// chain. Nothing typed here is trusted; the transaction itself is checked.
import { useCallback, useEffect, useState } from "react";
import { platform } from "../api.js";
import {
  Addr, Badge, Button, CopyBlock, CountUp, Empty, ErrorNote, Eyebrow, Icon, Input, Loading, Modal, Orb, PageHead, Panel, PanelHead, day, useApp, usd, when,
} from "../ui.jsx";

const SUB = {
  trial: { title: "Free trial", tone: "sky" },
  active: { title: "Active", tone: "lime" },
  expired: { title: "Ended", tone: "ember" },
};
const INVOICE_TONE = { paid: "lime", open: "amber" };
const MONTHS = [1, 3, 6, 12];
const months = (n) => `${n} ${n === 1 ? "month" : "months"}`;
// Whole days from now until a moment (0 once it has passed).
const daysTo = (iso) => Math.max(0, Math.ceil((new Date(iso).getTime() - Date.now()) / 86400000));

export default function Billing() {
  const { session, slug, notify, reloadMe } = useApp();
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  // false, true (a new payment), or an existing invoice whose payment is
  // being confirmed.
  const [paying, setPaying] = useState(false);
  const reload = useCallback(
    () =>
      platform(session, `/companies/${slug}/billing`).then(
        (d) => {
          setData(d);
          setError(null);
        },
        (e) => setError(e.message),
      ),
    [session, slug],
  );
  useEffect(() => {
    reload();
  }, [reload]);
  const sub = data?.subscription;
  const plan = data?.plan;

  return (
    <>
      <PageHead kicker={plan ? `Paid in NEAR · ${plan.network}` : "Paid in NEAR"} title="Billing" />
      <p className="reveal -mt-2 mb-6 max-w-2xl text-sm text-mute">One plan, paid in NEAR. Pay for one to twelve months at a time.</p>
      <ErrorNote error={error} />
      {!data && !error && <Loading rows={4} />}
      {data && (
        <div className="grid gap-4 md:grid-cols-5">
          <Panel className="flex min-h-80 flex-col p-6 sm:p-8 md:col-span-3">
            <Orb halted={sub.state === "expired"} className="pointer-events-none absolute -right-20 -top-16 w-64 opacity-50 sm:w-80" />
            <div className="relative flex flex-1 flex-col">
              <Eyebrow>Your subscription</Eyebrow>
              <div className="mt-4 flex flex-wrap items-center gap-3">
                <p className={`font-display text-4xl font-semibold tracking-tight sm:text-5xl ${sub.state === "expired" ? "text-ember" : ""}`}>{SUB[sub.state]?.title ?? sub.state}</p>
                <Badge tone={SUB[sub.state]?.tone} dot={sub.state !== "expired"}>{sub.state}</Badge>
              </div>
              {sub.state !== "expired" && (
                <p className="mt-6 flex items-end gap-3">
                  <span className="num font-display text-6xl font-semibold leading-none tracking-tighter">
                    <CountUp value={daysTo(sub.state === "trial" ? sub.ends_at : sub.paid_until)} />
                  </span>
                  <span className="pb-1 text-sm text-mute">{sub.state === "trial" ? "days left in your trial" : "days paid ahead"}</span>
                </p>
              )}
              <p className="mt-4 max-w-md text-sm text-mute">
                {sub.state === "trial" && `Your trial ends ${day(sub.ends_at)}. Paying now adds to the end of your trial.`}
                {sub.state === "active" && `Paid until ${day(sub.paid_until)}. Paying again extends it.`}
                {sub.state === "expired" && `Ended ${day(sub.since)}. Automation is paused until you renew; your data is kept.`}
              </p>
              <div className="mt-auto space-y-3 pt-8">
                <Button size="lg" onClick={() => setPaying(true)} disabled={!plan.pay_to}>
                  {sub.state === "active" ? "Extend with NEAR" : "Pay with NEAR"} <Icon name="arrow" size={16} />
                </Button>
                {!plan.pay_to && (
                  <p className="rounded-xl bg-amber/10 px-3 py-2 text-sm text-amber ring-1 ring-amber/30">Payments aren't set up on this platform yet.</p>
                )}
              </div>
            </div>
          </Panel>

          <Panel i={1} className="flex flex-col p-6 sm:p-8 md:col-span-2">
            <PanelHead eyebrow="Plan" title={`${plan.name} plan`} />
            <p className="num font-display text-5xl font-semibold tracking-tight">
              {usd(plan.price_usd)}
              <span className="ml-1.5 font-sans text-base font-normal tracking-normal text-mute">/ month</span>
            </p>
            <ul className="mt-6 space-y-3 text-sm text-mute">
              {[
                "AI workers, hiring, interviews, payroll, and performance reviews",
                "Your own jobs board and NEAR treasury",
                `${plan.trial_days}-day free trial for new companies`,
                `Paid in NEAR on ${plan.network} at the current rate`,
              ].map((t) => (
                <li key={t} className="flex items-start gap-2.5">
                  <Icon name="check" size={16} className="mt-0.5 shrink-0 text-lime" />
                  <span>{t}</span>
                </li>
              ))}
            </ul>
          </Panel>

          <Panel i={2} className="p-6 md:col-span-5">
            <PanelHead eyebrow="History" title="Invoices" />
            {data.invoices.length === 0 ? (
              <Empty title="No invoices yet." text="Each payment you start opens an invoice at the current NEAR rate." />
            ) : (
              <ul className="divide-y divide-white/7">
                {data.invoices.map((item) => {
                  const { invoice: i, amount_near } = item;
                  return (
                  <li key={i.id} className="flex items-start gap-4 py-4 first:pt-0 last:pb-0">
                    <span className={`flex shrink-0 items-center justify-center rounded-xl bg-white/5 size-10 ${i.status === "paid" ? "text-lime" : "text-mute"}`}>
                      <Icon name="billing" />
                    </span>
                    <div className="min-w-0 flex-1">
                      <p className="text-sm">
                        <span className="num text-base text-fog">{amount_near} NEAR</span> <span className="text-xs text-mute">({usd(i.amount_usd)})</span>
                      </p>
                      <p className="mt-0.5 text-xs text-mute">
                        {months(i.months)} · {when(i.created_at)}
                      </p>
                      {i.tx_hash && (
                        <p className="mt-1 flex min-w-0 items-center gap-1.5 text-xs text-dim">
                          <span className="shrink-0">Transaction</span>
                          <Addr value={i.tx_hash} className="text-mute" />
                        </p>
                      )}
                    </div>
                    {payable(i) && (
                      <Button size="sm" variant="ghost" className="shrink-0" onClick={() => setPaying(item)}>Confirm payment</Button>
                    )}
                    <Badge tone={INVOICE_TONE[i.status]} className="mt-1">{i.status}</Badge>
                  </li>
                  );
                })}
              </ul>
            )}
          </Panel>
        </div>
      )}
      {paying && (
        <PayDialog
          price={plan.price_usd}
          invoice={paying === true ? null : paying}
          onClose={() => setPaying(false)}
          onPaid={async (until) => {
            notify(`Payment confirmed. Paid until ${day(until)}.`);
            setPaying(false);
            await reload();
            reloadMe();
          }}
        />
      )}
    </>
  );
}

const STEPS = ["Choose how long", "Send the NEAR", "Confirm the payment"];

// A payment made inside an invoice's window still counts after a newer
// invoice replaced it, so it can be confirmed until shortly after it closes
// (the same few minutes the API allows).
const payable = (i) => i.status !== "paid" && new Date(i.expires_at).getTime() + 5 * 60000 > Date.now();

// `invoice`: an invoice opened earlier whose payment is being confirmed
// (the dialog starts at the last step); none for a new payment.
function PayDialog({ price, invoice, onClose, onPaid }) {
  const { session, slug } = useApp();
  const [step, setStep] = useState(invoice ? 2 : 0);
  const [count, setCount] = useState(1);
  const [inv, setInv] = useState(invoice || null);
  const [tx, setTx] = useState("");
  const [payer, setPayer] = useState("");
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(false);

  const open = async () => {
    setBusy(true);
    setError(null);
    try {
      setInv(await platform(session, `/companies/${slug}/billing/invoices`, { method: "POST", body: { months: count } }));
      setStep(1);
    } catch (e) {
      setError(e.message);
    }
    setBusy(false);
  };
  const verify = async (e) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const r = await platform(session, `/companies/${slug}/billing/invoices/${inv.invoice.id}/verify`, { method: "POST", body: { tx_hash: tx.trim(), payer: payer.trim() } });
      onPaid(r.paid_until);
    } catch (err) {
      setError(err.message);
    }
    setBusy(false);
  };

  // What was settled in each finished step.
  const recap = [inv ? `${months(inv.invoice.months)} · ${usd(inv.invoice.amount_usd)}` : null, inv ? `${inv.amount_near} NEAR` : null];

  return (
    <Modal open onClose={onClose} kicker="Billing" title="Pay with NEAR" backdropClose={false}>
      <ol>
        {STEPS.map((label, n) => {
          const done = n < step;
          const on = n === step;
          return (
            <li key={label} className="relative pl-11" aria-current={on ? "step" : undefined}>
              {n < STEPS.length - 1 && <span className={`absolute bottom-0 left-3.75 top-9 w-px ${done ? "bg-lime/50" : "bg-white/10"}`} aria-hidden="true" />}
              <span
                className={`absolute left-0 top-0 flex items-center justify-center rounded-full font-mono text-xs size-8 ${
                  done ? "bg-lime text-ink" : on ? "text-lime ring-2 ring-inset ring-lime" : "bg-white/5 text-dim"
                }`}
                aria-hidden="true"
              >
                {done ? <Icon name="check" size={14} /> : n + 1}
              </span>
              <p className={`pt-1.5 text-sm font-medium ${on ? "text-fog" : done ? "text-mute" : "text-dim"}`}>
                {label}
                {done && recap[n] && <span className="ml-2 font-mono text-xs font-normal text-dim">{recap[n]}</span>}
              </p>
              <div className="pb-6 pt-3">
                {on && n === 0 && (
                  <div className="space-y-4">
                    <div role="group" aria-label="Months" className="grid grid-cols-4 gap-2">
                      {MONTHS.map((m) => (
                        <button
                          type="button"
                          key={m}
                          aria-pressed={count === m}
                          onClick={() => setCount(m)}
                          className={`rounded-2xl px-1 py-3 text-center ring-1 ring-inset transition ${
                            count === m ? "bg-lime/10 text-lime ring-lime/50" : "bg-white/3 text-mute ring-white/10 hover:bg-white/6 hover:text-fog"
                          }`}
                        >
                          <span className="num block font-display text-2xl font-semibold leading-none">{m}</span>
                          <span className="mt-1 block text-[11px]">{m === 1 ? "month" : "months"}</span>
                        </button>
                      ))}
                    </div>
                    <p className="text-sm text-mute">
                      <span className="num text-fog">{usd(Number(price) * count)}</span>, paid in NEAR at the current rate.
                    </p>
                    <Button onClick={open} disabled={busy}>{busy ? "Getting the rate…" : "Get the NEAR amount"}</Button>
                  </div>
                )}
                {on && n === 1 && inv && (
                  <div className="space-y-3">
                    <p className="text-sm text-mute">
                      Send <b className="font-medium text-fog">exactly</b> this amount, in one transfer, from any NEAR wallet on <b className="font-medium text-fog">{inv.invoice.network}</b> before {when(inv.invoice.expires_at)}. It is {usd(inv.invoice.amount_usd)} at {Number(inv.invoice.usd_per_near)} USD/NEAR.
                    </p>
                    <p className="rounded-xl bg-amber/10 px-3 py-2 text-sm text-amber ring-1 ring-amber/30">
                      The last digits identify your payment. A rounded or different amount can't be matched to this invoice, so copy it as it is.
                    </p>
                    <CopyBlock label="Amount (NEAR)" value={inv.amount_near} big />
                    <CopyBlock label="Send to" value={inv.invoice.pay_to} />
                    <div className="pt-1">
                      <Button onClick={() => setStep(2)}>I've sent it</Button>
                    </div>
                  </div>
                )}
                {on && n === 2 && (
                  <form onSubmit={verify} className="space-y-4">
                    <p className="text-sm text-mute">Paste the transaction hash from your wallet and the account you sent from. ForeMan checks the transaction on NEAR.</p>
                    <Input label="Transaction hash" value={tx} onChange={setTx} autoComplete="off" spellCheck={false} className="[&_input]:font-mono" autoFocus />
                    <Input label="Sent from (NEAR account)" placeholder="yourname.testnet" value={payer} onChange={setPayer} autoComplete="off" spellCheck={false} />
                    <div className="flex flex-wrap items-center gap-2">
                      <Button type="submit" disabled={busy || !tx.trim() || !payer.trim()}>{busy ? "Checking on NEAR…" : "Confirm payment"}</Button>
                      <Button type="button" variant="ghost" onClick={() => setStep(1)} disabled={busy}>Back to the amount</Button>
                    </div>
                  </form>
                )}
              </div>
            </li>
          );
        })}
      </ol>
      <ErrorNote error={error} />
      <div className="mt-4 flex justify-end">
        <Button variant="ghost" onClick={onClose}>Close</Button>
      </div>
    </Modal>
  );
}
