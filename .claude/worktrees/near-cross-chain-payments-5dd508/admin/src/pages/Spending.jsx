// The monthly payout cap: what's sent, reserved, and left; and changing it.
import { useState } from "react";
import { Button, CountUp, ErrorNote, Icon, Input, Loading, Modal, PageHead, Panel, pctOf, useApp, useCompanyApi, useLoad, useThreshold, usd } from "../ui.jsx";

// A cap the API accepts: a positive amount with at most two decimals.
const VALID_CAP = /^\d+(\.\d{1,2})?$/;

// How far through the UTC month we are, for the pace marker.
function monthProgress(month) {
  const [y, m] = String(month || "").split("-").map(Number);
  if (!y || !m) return null;
  const now = new Date();
  const days = new Date(Date.UTC(y, m, 0)).getUTCDate();
  if (now.getUTCFullYear() !== y || now.getUTCMonth() + 1 !== m) return null;
  return { pct: ((now.getUTCDate() - 1 + now.getUTCHours() / 24) / days) * 100, day: now.getUTCDate(), days };
}

export default function Spending() {
  const { navigate } = useApp();
  const { data, error, reload } = useLoad("/admin/cap");
  const threshold = useThreshold();
  const [changing, setChanging] = useState(false);
  const n = (k) => Number(data?.[k] ?? 0);

  const cap = n("cap_usd");
  const sent = n("committed_usd");
  const reserved = n("reserved_usd");
  const pending = n("pending_approval_usd");
  const inProgress = Math.max(0, reserved - pending);
  const remaining = n("remaining_usd");
  const usedPct = pctOf(sent + reserved, cap);
  const pace = data && monthProgress(data.month);
  const ahead = pace && usedPct > pace.pct + 15;
  const monthName = data ? new Date(`${String(data.month).slice(0, 7)}-01T00:00:00Z`).toLocaleString(undefined, { month: "long", year: "numeric", timeZone: "UTC" }) : "";
  const count = data?.pending_approval_count ?? 0;

  // The meter: sent, reserved for payouts in progress, and the part of the
  // reservation that waits for approval.
  const segments = [
    { key: "sent", label: "Sent", value: sent, color: "bg-lime" },
    { key: "reserved", label: "Reserved, in progress", value: inProgress, color: "bg-sky" },
    { key: "pending", label: "Reserved, waiting for approval", value: pending, color: "bg-amber" },
  ];

  return (
    <>
      <PageHead kicker={data ? `This month · ${monthName}, UTC` : "This month · UTC"} title="Monthly cap">
        <Button variant="ghost" onClick={() => setChanging(true)} disabled={!data}>Change cap</Button>
      </PageHead>
      <ErrorNote error={error} />
      {!data && !error && <Loading rows={5} />}
      {data && (
        <div className="space-y-4">
          <Panel className="p-6 sm:p-8">
            <div className="flex flex-wrap items-end gap-x-10 gap-y-4">
              <div>
                <p className="text-sm text-mute">Left to pay out</p>
                <p className="num mt-1 font-display text-6xl font-semibold leading-none tracking-tight sm:text-7xl">
                  <CountUp value={remaining} format={(v) => usd(v, 0)} />
                </p>
              </div>
              <div className="pb-1 text-sm">
                <p className="text-mute">left of <span className="num font-medium text-fog">{usd(cap)}</span></p>
                <p className="mt-1 text-mute"><span className="num font-medium text-fog">{Math.round(usedPct)}%</span> sent or reserved</p>
              </div>
              {pace && (
                <p className={`rounded-full px-3 py-1.5 text-xs font-medium ring-1 ring-inset sm:ml-auto ${ahead ? "bg-amber/10 text-amber ring-amber/30" : "bg-lime/10 text-lime ring-lime/30"}`}>
                  {ahead ? "Spending ahead of pace" : "On pace"} · day {pace.day} of {pace.days}
                </p>
              )}
            </div>

            {/* Budget bar with a "today" marker. */}
            <div className="relative mt-10">
              {pace && (
                <div className="absolute -top-6 bottom-0 z-10" style={{ left: `${pace.pct}%` }} aria-hidden="true">
                  <span className="absolute -top-0.5 -translate-x-1/2 whitespace-nowrap font-mono text-[10px] uppercase tracking-wider text-fog">Today</span>
                  <span className="absolute top-4 bottom-0 w-px -translate-x-1/2 bg-fog" />
                </div>
              )}
              <div className="flex h-12 w-full overflow-hidden rounded-2xl bg-white/6 ring-1 ring-inset ring-white/10" role="img" aria-label={`Cap meter: ${usd(sent)} sent, ${usd(reserved)} reserved, ${usd(remaining)} left of ${usd(cap)}`}>
                {segments.map((s, i) =>
                  s.value > 0 ? (
                    <div key={s.key} className={`grow-x ${s.color} h-full border-r-2 border-ink last:border-r-0`} style={{ width: `${pctOf(s.value, cap)}%`, "--i": i }} title={`${s.label}: ${usd(s.value)}`} />
                  ) : null,
                )}
              </div>
              <div className="mt-2 flex justify-between font-mono text-[11px] text-mute">
                <span>$0</span>
                <span>{usd(cap / 2, 0)}</span>
                <span>{usd(cap, 0)}</span>
              </div>
            </div>
          </Panel>

          <Panel i={1} className="p-2 sm:p-3">
            <ul className="divide-y divide-white/7">
              <Row color="bg-lime" label="Sent (committed)" note="Paid out on NEAR this month" value={sent} cap={cap} />
              <Row color="bg-sky" label="Reserved for payouts in progress" note="Held against the cap until sent or released" value={reserved} cap={cap} />
              <Row
                sub
                color="bg-amber"
                label="…of which waiting for approval"
                note={`${count} payout${count === 1 ? "" : "s"} of ${usd(threshold)} or more`}
                value={pending}
                cap={cap}
                onOpen={pending > 0 || count > 0 ? () => navigate("approvals") : null}
              />
              <Row color="bg-white/15" label="Remaining" note="Still available this month" value={remaining} cap={cap} strong />
            </ul>
          </Panel>

          <p className="px-1 text-xs text-mute">
            A hard monthly limit on everything paid out, network fees included. Approving a payout never overrides it. Only an MFA-verified admin can change it, and every change is audited.
          </p>
        </div>
      )}
      <ChangeCap open={changing} onClose={() => setChanging(false)} data={data} onDone={reload} />
    </>
  );
}

function Row({ color, label, note, value, cap, strong, sub, onOpen }) {
  const inner = (
    <>
      <span className={`shrink-0 rounded-sm size-3 ${color}`} />
      <span className="min-w-0 flex-1">
        <span className={`block text-sm ${strong ? "font-semibold" : "font-medium"}`}>{label}</span>
        <span className="block text-xs text-mute">{note}</span>
      </span>
      <span className="num hidden w-12 text-right text-xs text-mute sm:block">{Math.round(pctOf(value, cap))}%</span>
      <span className={`num text-right text-base ${strong ? "font-semibold text-fog" : ""}`}>{usd(value)}</span>
      {/* Every row keeps the arrow's space so the amounts line up. */}
      <Icon name="arrow" size={14} className={`shrink-0 text-dim ${onOpen ? "" : "invisible"}`} />
    </>
  );
  const cls = `flex items-center gap-4 px-4 py-4 ${sub ? "pl-8 sm:pl-12" : ""}`;
  return (
    <li>
      {onOpen ? (
        <button onClick={onOpen} className={`${cls} w-full rounded-2xl text-left transition hover:bg-white/4`}>{inner}</button>
      ) : (
        <div className={cls}>{inner}</div>
      )}
    </li>
  );
}

function ChangeCap({ open, onClose, data, onDone }) {
  const { notify } = useApp();
  const call = useCompanyApi();
  const [cap, setCap] = useState("");
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  if (!data) return null;
  const validCap = VALID_CAP.test(cap) && Number(cap) > 0;
  const belowSent = validCap && Number(cap) < Number(data.committed_usd);
  const submit = async (e) => {
    e.preventDefault();
    if (!validCap || !reason.trim()) return;
    setBusy(true);
    try {
      await call("/admin/cap", { method: "POST", body: { new_cap_usd: cap, reason } });
      notify("Cap updated. The change is in the audit log.");
      setCap("");
      setReason("");
      onDone();
      onClose();
    } catch (err) {
      notify(err.message, "error");
    }
    setBusy(false);
  };
  return (
    <Modal open={open} onClose={onClose} kicker="Admin only · MFA" title="Change the monthly cap">
      <form onSubmit={submit} className="space-y-4">
        <p className="text-sm text-mute">Takes effect immediately. Every change is audited.</p>
        <div className="flex items-center justify-between rounded-2xl bg-white/4 px-4 py-3 text-sm ring-1 ring-white/10">
          <span className="text-mute">Current cap</span>
          <span className="num font-medium">{usd(data.cap_usd)}</span>
        </div>
        <Input
          label="New monthly cap (USD)"
          value={cap}
          onChange={(v) => setCap(v.replace(/[^\d.]/g, "").replace(/(\..*)\./g, "$1"))}
          inputMode="decimal"
          placeholder={String(data.cap_usd)}
          aria-invalid={!!cap && !validCap}
          hint={cap && !validCap ? <span className="text-ember">A positive amount with at most two decimals.</span> : null}
          className="[&_input]:font-mono [&_input]:text-xl"
          autoFocus
        />
        <Input label="Reason" value={reason} onChange={setReason} placeholder="Launch week contractor push" hint="Kept in the audit log." />
        {belowSent ? (
          <p className="rounded-xl bg-amber/10 px-3 py-2 text-xs text-amber ring-1 ring-amber/30" role="alert">
            That's below the {usd(data.committed_usd)} already sent this month, so no new payouts would go out until next month.
          </p>
        ) : (
          <p className="rounded-xl bg-white/4 px-3 py-2 text-xs text-mute ring-1 ring-white/10">
            Lowering the cap below what's already sent stops new payouts for the rest of the month.
          </p>
        )}
        <div className="grid grid-cols-2 gap-3 pt-2">
          <Button type="button" variant="ghost" onClick={onClose}>Cancel</Button>
          <Button type="submit" disabled={busy || !validCap || !reason.trim()}>{busy ? "Saving…" : "Change cap"}</Button>
        </div>
      </form>
    </Modal>
  );
}
