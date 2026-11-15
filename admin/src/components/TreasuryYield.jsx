// Treasury yield on Rhea lending, shared by the Overview card and the full
// page: the data, the on/off switch, and how to read the signer's latest plan.
// The signer decides and moves by the policy's rules alone; the switch only
// saves the company's choice.
import { useState } from "react";
import { ErrorNote, Icon, Loading, Panel, PanelHead, Toggle, useApp, useCompanyApi, useLoad } from "../ui.jsx";

// yocto string -> NEAR number (display only).
export const nearOf = (y) => (y == null ? null : Number(BigInt(y) / 10n ** 20n) / 1e4);
export const fmt = (n, d = 2) => (n == null ? "—" : Number(n).toLocaleString(undefined, { maximumFractionDigits: d }));
export const pct = (apr) => (apr == null ? "—" : `${(Number(apr) * 100).toLocaleString(undefined, { maximumFractionDigits: 2 })}%`);

/** The yield status and the switch, for the card and the page. */
// Polls the database-only status often enough that "Turning on/off" clears
// soon after the signer's one-minute check.
export function useTreasuryYield({ poll = 15000 } = {}) {
  const { notify } = useApp();
  const call = useCompanyApi();
  const yieldData = useLoad("/admin/treasury/yield", { poll });
  const policy = useLoad("/admin/policy");
  const [busy, setBusy] = useState(false);
  // Saves only the yield choice; the company's other rules stay as stored.
  const toggle = async (on) => {
    const c = policy.data?.company;
    if (!c) return;
    setBusy(true);
    try {
      await call("/admin/policy", { method: "POST", body: { approval_threshold_usd: c.approval_threshold_usd ?? null, candidate_retention_days: c.candidate_retention_days ?? null, treasury_yield_enabled: on } });
      notify(on ? "Treasury yield is on. The signer supplies idle NEAR to Rhea within a minute." : "Treasury yield is off. The signer withdraws everything from Rhea within a minute.");
      yieldData.reload();
      policy.reload();
    } catch (err) {
      notify(err.message, "error");
    }
    setBusy(false);
  };
  return { ...yieldData, toggle, busy, canToggle: !busy && !!policy.data };
}

/** Balances from the latest plan, in NEAR. */
export function balancesOf(plan) {
  const pos = plan?.position;
  const liquid = nearOf(pos?.liquid_yocto) ?? 0;
  const wnear = nearOf(pos?.wnear_yocto) ?? 0;
  const supplied = nearOf(pos?.supplied_yocto) ?? 0;
  const total = liquid + wnear + supplied;
  return { liquid, wnear, supplied, total, floor: nearOf(plan?.floor_yocto), target: nearOf(plan?.target_yocto), apr: pos?.supply_apr };
}

const MOVING = { register: "Registering on Rhea", supply: "Supplying", withdraw: "Withdrawing", unwrap: "Unwrapping" };

/** One word for where things stand, a badge tone, and a plain sentence. */
export function stateOf(data) {
  const plan = data?.plan;
  if (!data) return null;
  if (!plan) return data.enabled ? { label: "Starting", tone: "sky", text: "Waiting for the signer's first check of the treasury." } : { label: "Off", tone: "mute", text: "Turn it on and NEAR payroll can't spend this month earns in Rhea lending." };
  const r = plan.reason || "";
  const b = balancesOf(plan);
  if (plan.action) return { label: MOVING[plan.action.kind] || "Moving", tone: "sky", text: r };
  // The switch is saved at once; the plan catches up on the signer's next check.
  const plannedOff = r.startsWith("treasury yield is off");
  if (data.enabled && plannedOff) return { label: "Turning on", tone: "sky", text: "Saved. On its next check, within a minute, the signer supplies what payroll can't spend this month." };
  if (!data.enabled) {
    return b.supplied >= 0.001
      ? { label: "Turning off", tone: "sky", text: "Saved. On its next check, within a minute, the signer withdraws everything from Rhea with interest." }
      : { label: "Off", tone: "mute", text: "Everything is liquid in the treasury." };
  }
  if (r.includes("fund the treasury")) return { label: "Needs funding", tone: "amber", text: `Liquid NEAR is below the floor payroll needs, and nothing is in Rhea to pull back. Add NEAR to the treasury or lower the monthly cap.` };
  if (r.startsWith("no fresh NEAR price")) return { label: "No price", tone: "amber", text: "No fresh NEAR price, so nothing new is supplied. Payouts already owed stay covered." };
  if (r.includes("paused")) return { label: "Rhea paused", tone: "amber", text: r };
  if (r.startsWith("holding")) {
    return b.supplied > 0
      ? { label: "Earning", tone: "lime", text: `${fmt(b.supplied, 3)} NEAR is earning in Rhea; ${fmt(b.liquid, 3)} NEAR stays liquid for payroll.` }
      : { label: "Holding", tone: "mute", text: `Not enough above the target to supply yet (moves start at ${data.rules?.min_move_near ?? "0.5"} NEAR).` };
  }
  return { label: "Holding", tone: "mute", text: r };
}

const DOT = { lime: "bg-lime", sky: "bg-sky", amber: "bg-amber", ember: "bg-ember", mute: "bg-white/30" };
// Sits in a box one text line tall, so it centers on the first line of
// whatever text it leads (use with leading-relaxed).
export function StateDot({ tone }) {
  return (
    <span className="flex h-[1.625em] shrink-0 items-center" aria-hidden="true">
      <span className={`block rounded-full size-2 ${DOT[tone] || DOT.mute} ${tone === "sky" ? "pulse-dot" : ""}`} />
    </span>
  );
}

export function YieldSwitch({ y, small = true }) {
  if (!y.data) return null;
  return (
    <span className="flex items-center gap-2 text-xs text-mute">
      {y.data.enabled ? "On" : "Off"}
      <Toggle on={y.data.enabled} onChange={y.toggle} disabled={!y.canToggle} label="Treasury yield" small={small} />
    </span>
  );
}

/** The Overview card: the two balances, the split, one status line. */
export function TreasuryYieldCard({ i = 0, className = "" }) {
  const { navigate } = useApp();
  const y = useTreasuryYield();
  const { data, error } = y;
  const plan = data?.plan;
  const b = balancesOf(plan);
  const share = b.total > 0 ? Math.round((b.supplied / b.total) * 100) : 0;
  const state = stateOf(data);
  return (
    <Panel i={i} className={`flex flex-col p-6 ${className}`}>
      <PanelHead eyebrow="Rhea Finance lending" title="Treasury yield" action={<YieldSwitch y={y} />} />
      <ErrorNote error={error} />
      {!data && !error && <Loading rows={2} />}
      {plan && (
        <>
          <div className="grid grid-cols-2 gap-4">
            <div>
              <p className="text-xs text-mute">Earning in Rhea</p>
              <p className="num mt-1 font-display text-3xl font-semibold tracking-tight">{fmt(b.supplied)}<span className="ml-1.5 font-sans text-sm font-normal text-mute">NEAR</span></p>
            </div>
            <div>
              <p className="text-xs text-mute">Liquid for payroll</p>
              <p className="num mt-1 font-display text-3xl font-semibold tracking-tight">{fmt(b.liquid + b.wnear)}<span className="ml-1.5 font-sans text-sm font-normal text-mute">NEAR</span></p>
            </div>
          </div>
          {b.total > 0 && (
            <div className="mt-4 flex h-1.5 overflow-hidden rounded-full bg-white/5" role="img" aria-label={`${share}% of the treasury is in Rhea`}>
              <div className="bg-lime" style={{ width: `${share}%` }} />
            </div>
          )}
        </>
      )}
      {state && (
        <p className="mt-4 flex items-start gap-2 text-xs leading-relaxed text-mute">
          <StateDot tone={state.tone} />
          <span className="line-clamp-2" title={plan?.reason}><span className="font-medium text-fog">{state.label}.</span> {state.text}</span>
        </p>
      )}
      {data && (
        <button type="button" onClick={() => navigate("yield")} className="mt-auto inline-flex items-center gap-1 self-start pt-4 text-xs text-mute hover:text-fog">
          Details and history <Icon name="arrow" size={12} />
        </button>
      )}
    </Panel>
  );
}
