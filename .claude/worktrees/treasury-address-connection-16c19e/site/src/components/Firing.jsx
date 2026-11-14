import { useState } from "react";
import { Check, Gavel, X } from "lucide-react";
import { firingEligible } from "../lib/axom.js";
import { FIRING } from "../lib/config.js";
import { Range } from "./ui.jsx";

const PERIODS = ["Week 1", "Week 2", "Week 3", "Week 4"];

export default function Firing() {
  const [completed, setCompleted] = useState(14);
  const [days, setDays] = useState(21);
  const [weeks, setWeeks] = useState([true, true, false, false]); // true = met thresholds

  // Consecutive below-threshold periods, counted back from the latest.
  let bad = 0;
  for (let i = weeks.length - 1; i >= 0 && !weeks[i]; i--) bad++;
  const r = firingEligible({ completed, days, badPeriods: bad });

  return (
    <div className="panel demo spot" data-reveal="zoom">
      <div className="demo-controls">
        <span className="demo-tag"><Gavel aria-hidden="true" /> An AI worker's record</span>
        <Range id="done" label="Completed tasks" value={completed} min={0} max={40} onChange={setCompleted} />
        <Range id="days" label="Days of history" value={days} min={0} max={60} onChange={setDays} />
        <div>
          <div className="lbl small muted">Weekly reviews (click to flip)</div>
          <div className="grid-4 mt-s" style={{ gap: 8 }}>
            {PERIODS.map((p, i) => (
              <button
                key={p}
                type="button"
                className={`btn btn-sm ${weeks[i] ? "btn-ghost" : "btn-ember"}`}
                aria-pressed={!weeks[i]}
                onClick={() => setWeeks((w) => w.map((x, j) => (j === i ? !x : x)))}
                style={{ flexDirection: "column", gap: 4, minHeight: 64 }}
              >
                <span className="tiny" style={{ opacity: 0.8 }}>{p}</span>
                <span>{weeks[i] ? "Met" : "Below"}</span>
              </button>
            ))}
          </div>
        </div>
        <p className="tiny dim">
          A review is below threshold if any objective metric misses: completion under {FIRING.minCompletionRate * 100}%, on time under {FIRING.minOnTimeRate * 100}%, cost over ${FIRING.maxCostPerCompletedUsd} per completed task, or checked quality under {FIRING.minObjectiveQuality * 100}%. An LLM's opinion of the work is recorded but never counts.
        </p>
      </div>
      <div className="demo-out" aria-live="polite">
        <div className={`verdict ${r.fire ? "blocked" : "sent"}`} key={String(r.fire)} style={{ animation: "pop 0.5s var(--ease)" }}>
          <div className="v-k">Decision</div>
          <div className="v-t">{r.fire ? "Fired by AI, board notified" : "Keeps working"}</div>
          <div className="v-r">
            {r.fire
              ? "Open tasks are reassigned to other workers. The board gets the reason, the metrics and an audit link within a minute."
              : bad > 0
                ? "Below threshold but not eligible yet: the worker is put on probation and reviewed again."
                : "Performance is within thresholds."}
          </div>
        </div>
        <ul className="checks" key={r.checks.map((c) => c.ok).join()}>
          {r.checks.map((c, i) => (
            <li key={c.label} className={c.ok ? "pass" : "todo"} style={{ "--i": i, opacity: 1 }}>
              <span className="st">{c.ok ? <Check aria-hidden="true" /> : <X aria-hidden="true" />}</span>
              <div>
                <b>{c.label}</b>
                <span>{c.value}</span>
              </div>
            </li>
          ))}
        </ul>
        <p className="tiny dim">All three must hold. Measured from results (tasks done, deadlines met, cost, checks passed), never from self-reports.</p>
      </div>
    </div>
  );
}
