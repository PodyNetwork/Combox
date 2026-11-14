// New company: name and address, then the plan ForeMan builds the org chart
// from, then next steps. The free trial starts when the company is created.
import { useState } from "react";
import { platform } from "../api.js";
import { AuthShell } from "../Auth.jsx";
import PlanForm from "../components/PlanForm.jsx";
import { Button, ErrorNote, Icon, Input } from "../ui.jsx";

const slugify = (s) => s.toLowerCase().trim().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40);
// While typing the address, keep a trailing dash so "my-co" can be typed.
const slugTyping = (s) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+/, "").slice(0, 40);

const STEPS = ["Company", "Plan", "Ready"];
const NEXT = [
  { icon: "integrations", title: "Integrations", text: "Connect email, contracts (DocuSeal), social, and CRM." },
  { icon: "settings", title: "Settings", text: "Create your NEAR treasury and fund it to pay workers." },
  { icon: "members", title: "Members", text: "Invite your co-founders or HR team." },
];

export default function Onboarding({ session, onDone, canCancel, onCancel }) {
  const [step, setStep] = useState(0);
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugTouched, setSlugTouched] = useState(false);
  const [created, setCreated] = useState(null);
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(false);
  const [plan, setPlan] = useState(null);

  const create = async (e) => {
    e.preventDefault();
    const address = slugify(slug);
    setSlug(address);
    if (address.length < 3) return;
    setBusy(true);
    setError(null);
    try {
      const r = await platform(session, "/companies", { method: "POST", body: { name, slug: address } });
      setCreated(r);
      setStep(1);
    } catch (err) {
      setError(err.message);
    }
    setBusy(false);
  };
  const finish = async () => {
    setBusy(true);
    try {
      await onDone(created.company.slug);
    } finally {
      setBusy(false);
    }
  };

  const title = step === 0 ? "Create your company" : step === 1 ? "What should the company achieve?" : `${created.company.name} is ready`;
  return (
    <AuthShell step={`Step ${step + 1} of ${STEPS.length} · ${STEPS[step]}`} title={title}>
      {step === 0 && (
        <form onSubmit={create} className="space-y-5">
          <p className="text-sm text-mute">Your company gets its own private workspace and database. The free trial starts now; no payment needed.</p>
          <Input
            label="Company name"
            value={name}
            onChange={(v) => {
              setName(v);
              if (!slugTouched) setSlug(slugify(v));
            }}
            autoComplete="organization"
            required
            autoFocus
          />
          <div>
            <label className="block text-sm">
              <span className="text-mute">Address</span>
              <span className="mt-1.5 flex items-center rounded-xl border border-white/10 bg-black/40 transition focus-within:border-lime/60 focus-within:ring-4 focus-within:ring-lime/10">
                <span className="pl-3.5 font-mono text-sm text-dim" aria-hidden="true">/t/</span>
                <input
                  value={slug}
                  onChange={(e) => {
                    setSlugTouched(true);
                    setSlug(slugTyping(e.target.value));
                  }}
                  aria-describedby="onboarding-address-hint"
                  autoCapitalize="none"
                  autoComplete="off"
                  spellCheck={false}
                  required
                  className="min-w-0 flex-1 bg-transparent py-2.5 pl-0.5 pr-3.5 font-mono text-sm text-fog placeholder:text-dim focus:outline-none focus-visible:outline-none"
                />
              </span>
            </label>
            <p id="onboarding-address-hint" className="mt-1 text-xs text-dim">
              Used in links, such as your public jobs board. Lowercase letters, digits, and dashes.
            </p>
          </div>
          <ErrorNote error={error} />
          <div className="flex gap-3 pt-1">
            {canCancel && (
              <Button type="button" variant="ghost" size="lg" onClick={onCancel}>
                Cancel
              </Button>
            )}
            <Button type="submit" size="lg" className="flex-1" disabled={busy || !name.trim() || slugify(slug).length < 3}>
              {busy ? "Creating…" : "Create company"}
            </Button>
          </div>
        </form>
      )}

      {step === 1 && (
        <div className="space-y-5">
          <p className="text-sm text-mute">
            ForeMan turns your goal and objectives into departments, AI workers, and tasks, and opens hiring requests for work that needs people. You can skip this and do it later in Settings.
          </p>
          <PlanForm
            session={session}
            slug={created.company.slug}
            onSaved={(r) => {
              setPlan(r);
              setStep(2);
            }}
            onSkip={() => setStep(2)}
          />
        </div>
      )}

      {step === 2 && (
        <div className="space-y-5">
          {plan && (
            <p className="rounded-xl bg-lime/10 px-3 py-2 text-sm text-lime ring-1 ring-lime/30">
              Created {plan.departments} departments, {plan.workers_created} AI workers, and {plan.tasks_created} tasks.
            </p>
          )}
          <p className="text-sm text-mute">
            Your free trial ends <span className="text-fog">{new Date(created.company.trial_ends_at).toLocaleDateString(undefined, { dateStyle: "long" })}</span>. Next:
          </p>
          <ul className="space-y-2">
            {NEXT.map((n) => (
              <li key={n.title} className="flex items-start gap-3 rounded-2xl bg-white/3 p-3 ring-1 ring-white/10">
                <span className="flex shrink-0 items-center justify-center rounded-xl bg-white/5 text-lime size-9">
                  <Icon name={n.icon} size={16} />
                </span>
                <span className="min-w-0 text-sm">
                  <b className="block font-medium text-fog">{n.title}</b>
                  <span className="block text-xs text-mute">{n.text}</span>
                </span>
              </li>
            ))}
          </ul>
          <Button size="lg" className="w-full" onClick={finish} disabled={busy}>
            {busy ? "Opening…" : "Go to dashboard"} <Icon name="arrow" size={16} />
          </Button>
        </div>
      )}
    </AuthShell>
  );
}
