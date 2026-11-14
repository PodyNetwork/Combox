// One switch halts every tool call and payout, for this company only. The
// state comes from the shell (polled), so the hazard banner and this page
// always agree.
import { useState } from "react";
import { Button, ErrorNote, Eyebrow, Icon, Input, KineticText, Loading, Modal, Orb, ago, useApp, useCompanyApi, when } from "../ui.jsx";

export default function KillSwitch() {
  const { notify, refreshStatus, status } = useApp();
  const call = useCompanyApi();
  const { data, error } = status.kill;
  const [confirming, setConfirming] = useState(false);
  const [reason, setReason] = useState("");
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const on = !!data?.on;

  const closeConfirm = () => {
    setConfirming(false);
    setTyped("");
    setReason("");
  };
  const set = async (next) => {
    setBusy(true);
    try {
      await call("/admin/kill-switch", {
        method: "POST",
        body: { on: next, reason: reason.trim() || (next ? "halted from dashboard" : "resumed from dashboard") },
      });
      notify(next ? "Everything is halted." : "Operations resumed.", next ? "warning" : "success");
      closeConfirm();
    } catch (e) {
      notify(e.message, "error");
    }
    setBusy(false);
    refreshStatus();
  };

  return (
    <div className="relative">
      <ErrorNote error={error} />
      {!data && !error && <Loading rows={6} />}
      {data && (
        <section
          aria-label="Kill switch"
          className={`reveal relative overflow-hidden rounded-4xl border p-6 pb-24 sm:p-12 sm:pb-28 ${on ? "border-ember/40 bg-ember/4" : "border-white/5 bg-panel"}`}
        >
          {on && <div className="hazard hazard-move absolute inset-x-0 top-0 h-3" />}
          <Orb halted={on} className="pointer-events-none absolute -right-24 top-1/2 w-80 -translate-y-1/2 opacity-60 sm:w-120" />
          <div className={`horizon ${on ? "" : "horizon-lime"}`} style={{ "--top": "calc(100% - 3.5rem)", width: "130%" }} aria-hidden="true" />
          <div className="relative max-w-2xl">
            <Eyebrow className={on ? "text-ember opacity-100" : "text-lime opacity-100"}>Kill switch · {on ? "ON: everything halted" : "Off: operating normally"}</Eyebrow>
            <h1 className={`mt-4 font-display text-5xl font-bold leading-[0.9] tracking-tighter sm:text-8xl ${on ? "text-ember" : ""}`}>
              <KineticText key={String(on)} text={on ? "Halted." : "Operating."} />
            </h1>
            <p className="mt-6 max-w-lg text-mute">
              {on
                ? "Every tool call and every payout is stopped. Nothing moves money, hires, fires or sends until you resume."
                : "Stops every tool call and payout immediately, for this company only. Use it if anything looks wrong."}
            </p>

            <button
              onClick={() => (on ? set(false) : setConfirming(true))}
              disabled={busy}
              aria-pressed={on}
              aria-label={on ? "Resume operations" : "Halt everything"}
              className={`group relative mt-10 flex h-24 w-full max-w-md items-center rounded-full p-2 transition-colors duration-500 disabled:opacity-60 ${on ? "hazard hazard-move" : "bg-white/6 ring-1 ring-white/10 hover:bg-white/9"}`}
            >
              <span
                style={{ left: on ? "calc(100% - 5.5rem)" : "0.5rem" }}
                className={`absolute top-2 flex items-center justify-center rounded-full text-xs font-bold shadow-2xl transition-all duration-700 ease-[cubic-bezier(.2,.8,.2,1)] size-20 ${on ? "bg-ink text-ember ring-4 ring-ember" : "bg-lime text-ink group-hover:scale-105"}`}
              >
                <Icon name="kill" size={28} />
              </span>
              {!on && <span className="ml-24 mr-auto pl-4 font-display text-sm font-semibold tracking-widest text-ember">HALT EVERYTHING →</span>}
            </button>
            {on && <Button size="lg" className="mt-4" onClick={() => set(false)} disabled={busy}>{busy ? "Resuming…" : "Resume operations"}</Button>}
            <p className="mt-6 font-mono text-xs text-dim">Changed {ago(data.updated_at)} ({when(data.updated_at)}) by {data.updated_by}</p>
          </div>
        </section>
      )}

      <Modal open={confirming} onClose={closeConfirm} kicker="Are you sure?" title="Halt everything?">
        <div className="hazard mb-5 h-2 rounded-full" />
        <p className="mb-4 text-sm text-mute">All tool calls and payouts stop until you resume. Use this if something looks wrong. Agents stop mid-task and queued payouts stay queued.</p>
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (!busy && typed.trim().toUpperCase() === "HALT") set(true);
          }}
        >
          <Input label="Reason (audited)" value={reason} onChange={setReason} placeholder="halted from dashboard" hint="Optional. Kept in the audit log." />
          <Input label='Type "HALT" to confirm' value={typed} onChange={setTyped} autoComplete="off" className="[&_input]:font-mono [&_input]:uppercase" autoFocus />
          <div className="grid grid-cols-2 gap-3">
            <Button type="button" variant="ghost" onClick={closeConfirm}>Cancel</Button>
            <Button type="submit" variant="danger" disabled={busy || typed.trim().toUpperCase() !== "HALT"}>
              {busy ? "Halting…" : "Halt everything"}
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
}
