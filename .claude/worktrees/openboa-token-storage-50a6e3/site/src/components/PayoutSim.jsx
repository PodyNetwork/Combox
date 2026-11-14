import { useMemo, useState } from "react";
import { Check, Hourglass, Minus, SlidersHorizontal, X } from "lucide-react";
import { evaluatePayout } from "../lib/axom.js";
import { POLICY } from "../lib/config.js";
import { usd } from "../lib/hooks.js";
import { Range, Toggle } from "./ui.jsx";

const PRESETS = [
  { label: "$5 weekly pay", v: { amount: 5, window24h: 0, monthUsed: 180, address: true, taxForm: true, priceFresh: true, killSwitch: false } },
  { label: "$60 invoice", v: { amount: 60, window24h: 0, monthUsed: 186, address: true, taxForm: true, priceFresh: true, killSwitch: false } },
  { label: "Split into $30 + $30", v: { amount: 30, window24h: 30, monthUsed: 216, address: true, taxForm: true, priceFresh: true, killSwitch: false } },
  { label: "$400 bonus", v: { amount: 400, window24h: 0, monthUsed: 247, address: true, taxForm: true, priceFresh: true, killSwitch: false } },
  { label: "New worker, no tax form", v: { amount: 20, window24h: 0, monthUsed: 100, address: true, taxForm: false, priceFresh: true, killSwitch: false } },
];

const TITLES = { sent: "Sent automatically", held: "Escalated to the board", blocked: "Blocked" };
const KICKER = { sent: "Outcome · on NEAR in seconds", held: "Outcome · waits in the dashboard (MFA)", blocked: "Outcome · nothing moves" };

export default function PayoutSim() {
  const [s, setS] = useState(PRESETS[1].v);
  const set = (k) => (v) => setS((p) => ({ ...p, [k]: v }));
  const r = useMemo(() => evaluatePayout(s), [s]);
  const key = JSON.stringify(s);
  // Re-animate only when an outcome changes, not on every slider tick.
  const sig = r.outcome + r.steps.map((st) => st.status).join();

  const fee = 0.01;
  const over = Math.max(0, s.monthUsed + s.amount + fee - POLICY.capUsd);
  const fits = Math.max(0, s.amount - over);

  return (
    <div className="panel demo spot" data-reveal="zoom">
      <div className="demo-controls">
        <span className="demo-tag"><SlidersHorizontal aria-hidden="true" /> Try a payout</span>
        <div className="presets" role="group" aria-label="Example payouts">
          {PRESETS.map((p) => (
            <button key={p.label} type="button" className="chip" aria-pressed={JSON.stringify(p.v) === key} onClick={() => setS(p.v)}>
              {p.label}
            </button>
          ))}
        </div>
        <Range id="amt" label="This payout" value={s.amount} min={1} max={600} onChange={set("amount")} format={(v) => usd(v)} />
        <Range id="win" label="Already paid to them in the last 24h" value={s.window24h} min={0} max={100} onChange={set("window24h")} format={(v) => usd(v)} />
        <Range id="used" label="All payouts so far this month" value={s.monthUsed} min={0} max={500} onChange={set("monthUsed")} format={(v) => usd(v)} />
        <div className="toggles">
          <Toggle label="Worker proved their NEAR address" checked={s.address} onChange={set("address")} />
          <Toggle label="Tax form (W-9 / W-8BEN) on file" checked={s.taxForm} onChange={set("taxForm")} />
          <Toggle label="NEAR price is fresh" hint="Quotes older than 15 minutes force approval" checked={s.priceFresh} onChange={set("priceFresh")} />
          <Toggle label="Kill switch" checked={s.killSwitch} onChange={set("killSwitch")} />
        </div>
      </div>

      <div className="demo-out" aria-live="polite">
        <div key={`v-${sig}`} className={`verdict ${r.outcome} verdict-pop`}>
          <div className="v-k">{KICKER[r.outcome]}</div>
          <div className="v-t">{TITLES[r.outcome]}</div>
          <div className="v-r">{r.reason}</div>
        </div>

        <div>
          <div className="lbl small muted" style={{ display: "flex", justifyContent: "space-between" }}>
            <span>Monthly cap after this payout</span>
            <span className="mono">{usd(Math.min(POLICY.capUsd, s.monthUsed + s.amount + fee))} / {usd(POLICY.capUsd, 0)}</span>
          </div>
          <div className="meter" style={{ height: 10 }}>
            <i className="m-used" style={{ width: `${(Math.min(s.monthUsed, POLICY.capUsd) / POLICY.capUsd) * 100}%` }} />
            <i className="m-new" style={{ width: `${(Math.min(fits, POLICY.capUsd - s.monthUsed) / POLICY.capUsd) * 100}%` }} />
            {over > 0 && <i className="m-over" style={{ width: "6%" }} />}
          </div>
          <div className="legend">
            <span><i style={{ background: "var(--lime)" }} />Used</span>
            <span><i style={{ background: "var(--sky)" }} />This payout</span>
            {over > 0 && <span className="ember"><i style={{ background: "var(--ember)" }} />Over by {usd(over)}</span>}
          </div>
        </div>

        <ol className="checks" key={`c-${sig}`} aria-label="Checks, in order">
          {r.steps.map((st, i) => (
            <li key={st.key} className={st.status} style={{ "--i": i }}>
              <span className="st">
                {st.status === "pass" ? <Check aria-hidden="true" /> : st.status === "hold" ? <Hourglass aria-hidden="true" /> : st.status === "fail" ? <X aria-hidden="true" /> : <Minus aria-hidden="true" />}
              </span>
              <div>
                <b>{st.label}</b>
                <span>{st.detail}</span>
              </div>
            </li>
          ))}
        </ol>
        <p className="tiny dim">
          The same order of checks Combox runs. Policy {POLICY.version}. Network fees count toward the cap. Thresholds are platform defaults; a company can only make them stricter.
        </p>
      </div>
    </div>
  );
}
