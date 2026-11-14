import { useState } from "react";
import { ArrowRight, Check, Laptop, Server, Sparkles } from "lucide-react";
import { Faq, PageHero, Range, SectionHead } from "../components/ui.jsx";
import { DASHBOARD_HOME, PLAN_PRICE_USD, TRIAL_DAYS } from "../lib/config.js";

const INCLUDED = [
  ["AI strategy, org design and task assignment", true, true],
  ["An AI workforce in every department, with your Claude model and MCP tools", true, true],
  ["AI recruiting: ForeMan Jobs board and AI video interviews", true, true],
  ["Contracts, briefs portal and autonomous weekly NEAR payroll", true, true],
  ["Hard limits: policy engine, $50 board escalations, monthly cap, kill switch", true, true],
  ["Your own database and NEAR treasury", true, true],
  ["You run and update the servers", true, false],
  ["Many companies, invitations, operator view", false, true],
];

const FAQ = [
  { q: "Why is the subscription paid in NEAR?", a: "ForeMan already lives on NEAR: it's how your company's AI pays its workers. Paying the subscription the same way means no card details to store. You send NEAR from any wallet and paste the transaction hash; ForeMan checks the transaction on chain before extending your plan." },
  { q: "How is the NEAR amount worked out?", a: `The plan is priced in US dollars (${"$"}${PLAN_PRICE_USD} a month). At checkout ForeMan quotes that price in NEAR at the live rate. The calculator on this page is only an estimate at whatever rate you type in.` },
  { q: "What happens when the trial or a paid month ends?", a: "You get an email reminder 3 days before, and another when it ends. If it lapses, the company pauses (no tasks, hiring or payouts), but the dashboard stays open so you can renew. Nothing is deleted." },
  { q: "Does the subscription include what I pay my workers?", a: "No. Worker payouts come from your own NEAR treasury, which you fund from your own wallet. ForeMan never holds customer money. The same goes for tool costs such as your Claude API key." },
  { q: "Is there a free way to try it?", a: `Yes, two. Run the full demo on your own laptop with ./scripts/start demo (no accounts, NEAR testnet), or start a ${TRIAL_DAYS}-day free trial on the platform.` },
  { q: "Can the platform operator change the price?", a: "Yes. Whoever runs an ForeMan platform sets its price and trial length. The figures here are the defaults." },
];

export default function Pricing() {
  return (
    <>
      <PageHero eyebrow="Pricing" lines={["One plan.", "A whole company."]} lead={`Watch an autonomous company run for free, run your own on your servers, or let us host it. On the platform it's $${PLAN_PRICE_USD} a month after a ${TRIAL_DAYS}-day trial, for the entire AI-run company: every department, every agent.`} />

      <section className="section-tight">
        <div className="wrap">
          <div className="grid-3">
            <div className="panel pad-l price-card spot" data-reveal>
              <span className="card-icon sky"><Laptop aria-hidden="true" /></span>
              <h3 className="h-s">Demo</h3>
              <div className="price"><b>Free</b></div>
              <p className="muted small">A whole AI-run company on your laptop, with outside services mocked and real NEAR testnet payouts.</p>
              <ul className="check-list">
                <li><Check aria-hidden="true" /><span>One command: <span className="mono lime">./scripts/start demo</span></span></li>
                <li><Check aria-hidden="true" />No accounts or keys needed</li>
                <li><Check aria-hidden="true" />Sign in to the dashboard in one click</li>
              </ul>
              <a className="btn btn-ghost mt-s" href="/get-started/#demo">Run the demo</a>
            </div>
            <div className="panel pad-l price-card featured spot" data-reveal style={{ "--d": "0.06s" }}>
              <span className="badge badge-lime ribbon">Most popular</span>
              <span className="card-icon"><Sparkles aria-hidden="true" /></span>
              <h3 className="h-s">Platform</h3>
              <div className="price"><b>${PLAN_PRICE_USD}</b><span>per company / month</span></div>
              <p className="muted small">Launch an autonomous company on hosted ForeMan. {TRIAL_DAYS} days free, no payment to start.</p>
              <ul className="check-list">
                <li><Check aria-hidden="true" />Your own isolated database and treasury</li>
                <li><Check aria-hidden="true" />Invite your board (two-factor for all)</li>
                <li><Check aria-hidden="true" />Bring your own tools and keys, encrypted</li>
                <li><Check aria-hidden="true" />Reminders before renewal, never auto-charged</li>
              </ul>
              <a className="btn btn-lime mt-s magnetic" href={DASHBOARD_HOME}>Start the {TRIAL_DAYS}-day trial <ArrowRight className="arrow" aria-hidden="true" /></a>
            </div>
            <div className="panel pad-l price-card spot" data-reveal style={{ "--d": "0.12s" }}>
              <span className="card-icon violet"><Server aria-hidden="true" /></span>
              <h3 className="h-s">Self-hosted</h3>
              <div className="price"><b className="price-word">Your servers</b></div>
              <p className="muted small">Run your autonomous company, or a whole platform of them, on a VPS you control.</p>
              <ul className="check-list">
                <li><Check aria-hidden="true" />Docker Compose or Coolify definitions</li>
                <li><Check aria-hidden="true" />OpenBao, LiveKit, Postgres included</li>
                <li><Check aria-hidden="true" />You hold every key and log</li>
              </ul>
              <a className="btn btn-ghost mt-s" href="/get-started/#self-host">Self-hosting guide</a>
            </div>
          </div>
        </div>
      </section>

      <section className="section">
        <div className="wrap">
          <SectionHead eyebrow="Paying in NEAR" title="Five steps, no card on file." />
          <div className="grid-2" style={{ alignItems: "start" }}>
            <ol className="timeline" data-reveal="left">
              <li><span className="n">1</span><div><h4>Open Billing in the dashboard</h4><p>Pick 1 to 12 months. ForeMan quotes the price in NEAR at the live rate.</p></div></li>
              <li><span className="n">2</span><div><h4>Send NEAR from any wallet</h4><p>To the platform's billing account shown on the page.</p></div></li>
              <li><span className="n">3</span><div><h4>Paste the transaction hash</h4><p>That's the whole form.</p></div></li>
              <li><span className="n">4</span><div><h4>ForeMan checks it on chain</h4><p>Recipient, amount, success and timing are verified on NEAR itself before anything is extended.</p></div></li>
              <li><span className="n">5</span><div><h4>Your subscription is extended</h4><p>With an email reminder 3 days before it ends next time.</p></div></li>
            </ol>
            <Calculator />
          </div>
        </div>
      </section>

      <section className="section">
        <div className="wrap">
          <SectionHead eyebrow="Compare" title="What's in each." />
          <div className="table-wrap" data-reveal>
            <table className="table">
              <thead>
                <tr><th scope="col">Feature</th><th scope="col">Self-hosted</th><th scope="col">Platform</th></tr>
              </thead>
              <tbody>
                {INCLUDED.map(([f, a, b]) => (
                  <tr key={f}>
                    <td>{f}</td>
                    <td>{a ? <Check size={18} className="lime" aria-label="Included" /> : <span className="dim" aria-label="Not included">—</span>}</td>
                    <td>{b ? <Check size={18} className="lime" aria-label="Included" /> : <span className="dim" aria-label="Not included">—</span>}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="tiny dim mt-s">A self-hosted install can also run in platform mode (AXOM_MULTI_TENANT=true) to serve many companies.</p>
        </div>
      </section>

      <section className="section">
        <div className="wrap" style={{ maxWidth: 920 }}>
          <SectionHead center eyebrow="Billing questions" title="Good to know." />
          <Faq items={FAQ} />
        </div>
      </section>
    </>
  );
}

function Calculator() {
  const [rate, setRate] = useState(2.5);
  const [months, setMonths] = useState(1);
  const [workers, setWorkers] = useState(150);
  const near = (PLAN_PRICE_USD * months) / rate;
  return (
    <div className="panel pad-l spot stack" data-reveal="right" style={{ gap: 22 }}>
      <span className="demo-tag">Estimate</span>
      <Range id="rate" label="NEAR price (USD), your guess" value={rate} min={0.5} max={15} step={0.05} onChange={setRate} format={(v) => `$${v.toFixed(2)}`} />
      <Range id="months" label="Months to pay for" value={months} min={1} max={12} onChange={setMonths} format={(v) => `${v} ${v === 1 ? "month" : "months"}`} />
      <div>
        <div className="small muted">Subscription total</div>
        <div className="row" style={{ alignItems: "baseline", gap: 10, marginTop: 6 }}>
          <span style={{ font: "700 clamp(40px, 5vw, 56px)/1 var(--display)", letterSpacing: "-0.06em" }}>{near.toFixed(2)}</span>
          <span className="muted">NEAR</span>
        </div>
        <p className="tiny dim mt-s">${PLAN_PRICE_USD} × {months} = ${PLAN_PRICE_USD * months}, ÷ ${rate.toFixed(2)} per NEAR. The real quote uses the live rate at checkout.</p>
      </div>
      <div className="divider" />
      <Range id="payroll" label="Your monthly worker payouts (USD)" value={workers} min={0} max={500} step={10} onChange={setWorkers} format={(v) => `$${v}`} />
      <p className="small muted">
        Fund your treasury with about <b className="lime">{(workers / rate).toFixed(1)} NEAR</b> plus a little for fees. That money stays in the company's treasury and the AI pays it only to its workers, under your rules and cap.
      </p>
    </div>
  );
}
