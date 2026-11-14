// The company's goal, budget, deadline, and objectives. ForeMan turns them into
// departments, AI workers, tasks, and hiring requests.
import { useState } from "react";
import { platform } from "../api.js";
import DateTimePicker from "../DateTimePicker.jsx";
import { Button, ErrorNote, Icon, Input } from "../ui.jsx";

const inDays = (n) => new Date(Date.now() + n * 86400000).toISOString().slice(0, 10);

export default function PlanForm({ session, slug, onSaved, onSkip, submitLabel = "Build my team" }) {
  const [goal, setGoal] = useState("");
  const [budget, setBudget] = useState("450");
  const [deadline, setDeadline] = useState(inDays(90));
  const [objectives, setObjectives] = useState([{ title: "", description: "" }]);
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(false);
  const setObjective = (i, k, v) => setObjectives(objectives.map((o, j) => (j === i ? { ...o, [k]: v } : o)));

  const save = async (e) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const r = await platform(session, `/companies/${slug}/plan`, {
        method: "POST",
        body: { goal, budget_usd: budget, deadline, objectives: objectives.filter((o) => o.title.trim()) },
      });
      onSaved(r);
    } catch (err) {
      setError(err.message);
    }
    setBusy(false);
  };

  // A container query, so the form also fits the narrow onboarding column.
  return (
    <form onSubmit={save} className="@container space-y-5">
      <Input label="Goal" placeholder="Launch a SaaS product in 90 days" value={goal} onChange={setGoal} maxLength={300} required />
      <div className="grid grid-cols-1 gap-4 @lg:grid-cols-2">
        <Input label="Budget (USD)" type="number" value={budget} onChange={setBudget} min={1} step="any" inputMode="decimal" className="[&_input]:font-mono" required />
        <DateTimePicker label="Deadline" value={deadline} onChange={setDeadline} time={false} hint="Must be after today." />
      </div>

      <fieldset>
        <legend className="font-mono text-[11px] uppercase tracking-[0.18em] text-mute">Objectives</legend>
        <ol className="mt-3 space-y-3">
          {objectives.map((o, i) => (
            <li key={i} className="flex items-start gap-2 rounded-2xl bg-white/3 p-3 ring-1 ring-white/10 sm:p-4">
              <div className="min-w-0 flex-1 space-y-3">
                <Input label={`Objective ${i + 1}`} placeholder="Run the launch marketing campaign" value={o.title} onChange={(v) => setObjective(i, "title", v)} maxLength={200} />
                <Input label="Details (optional)" value={o.description} onChange={(v) => setObjective(i, "description", v)} maxLength={1000} />
              </div>
              <button
                type="button"
                aria-label={`Remove objective ${i + 1}`}
                title="Remove objective"
                onClick={() => setObjectives(objectives.filter((_, j) => j !== i))}
                disabled={objectives.length === 1}
                className="mt-6 shrink-0 rounded-full p-2 text-mute transition hover:bg-white/5 hover:text-fog disabled:pointer-events-none disabled:opacity-30"
              >
                <Icon name="x" size={16} />
              </button>
            </li>
          ))}
        </ol>
        <Button type="button" variant="ghost" size="sm" className="mt-3" onClick={() => setObjectives([...objectives, { title: "", description: "" }])} disabled={objectives.length >= 20}>
          <Icon name="plus" size={14} /> Add objective
        </Button>
      </fieldset>

      <ErrorNote error={error} />
      <div className="flex flex-wrap justify-end gap-3">
        {onSkip && <Button type="button" variant="ghost" onClick={onSkip}>Skip for now</Button>}
        <Button type="submit" disabled={busy || !goal.trim() || !deadline || !objectives.some((o) => o.title.trim())}>
          {busy ? "Building…" : submitLabel}
        </Button>
      </div>
    </form>
  );
}
