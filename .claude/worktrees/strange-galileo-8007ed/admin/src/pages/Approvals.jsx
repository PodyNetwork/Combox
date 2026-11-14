// Payouts at or above the approval threshold wait here for an MFA-verified admin.
import { useState } from "react";
import { Addr, Badge, Button, CountUp, ErrorNote, Eyebrow, Icon, KineticText, Loading, Modal, PageHead, Panel, PanelHead, StatusBadge, Textarea, ago, nearOf, pctOf, useApp, useCompanyApi, useLoad, useThreshold, usd, when } from "../ui.jsx";

// Well-known NEAR Intents asset ids -> symbol. Anything else shows the raw id.
const KNOWN_ASSETS = {
  "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near": "USDC",
  "nep141:eth-0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48.omft.near": "USDC",
  "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near": "USDC",
  "nep141:sol-5ce3bf3a31af18be40ba30f721101b4341690186.omft.near": "USDC",
  "nep141:17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1": "USDC",
  "nep141:eth.omft.near": "ETH",
  "nep141:base.omft.near": "ETH",
  "nep141:arb.omft.near": "ETH",
  "nep141:btc.omft.near": "BTC",
  "nep141:sol.omft.near": "SOL",
};
const assetLabel = (id) => KNOWN_ASSETS[id] || id;
const destLabel = (d) => `${assetLabel(d.asset_id)} on ${d.chain_name || d.chain}`;
const nearblocks = (network, hash) => `https://${network === "mainnet" ? "" : "testnet."}nearblocks.io/txns/${hash}`;

export default function Approvals() {
  const { notify, refreshStatus, navigate } = useApp();
  const call = useCompanyApi();
  const { data, error, loading, reload } = useLoad("/payouts/pending");
  const cap = useLoad("/admin/cap");
  const threshold = useThreshold();
  const [busy, setBusy] = useState(null);
  const [rejecting, setRejecting] = useState(null);
  const [approving, setApproving] = useState(null);
  const list = data?.payouts || [];
  const total = list.reduce((s, p) => s + Number(p.amount_usd), 0);

  // Approve or reject; the dialogs stay open if the API refuses.
  const act = async (p, action, body = {}) => {
    setBusy(p.id);
    try {
      await call(`/payouts/${p.id}/${action}`, { method: "POST", body });
      notify(action === "approve" ? "Approved. The signer will send it." : "Rejected.");
      setApproving(null);
      setRejecting(null);
    } catch (e) {
      notify(e.message, "error");
    }
    setBusy(null);
    reload();
    cap.reload();
    refreshStatus();
  };

  return (
    <>
      <PageHead kicker={`Payouts of ${usd(threshold)} or more wait for you`} title="Payroll">
        {cap.data && (
          <button onClick={() => navigate("spending")} className="w-full max-w-xs rounded-2xl bg-white/3 p-4 text-left ring-1 ring-white/10 transition hover:bg-white/5 sm:w-72">
            <span className="flex justify-between text-xs text-mute"><span>Cap left this month</span><span className="num text-fog">{usd(cap.data.remaining_usd)}</span></span>
            <span className="mt-2 flex h-1.5 overflow-hidden rounded-full bg-white/5" role="img" aria-label={`${usd(cap.data.remaining_usd)} of ${usd(cap.data.cap_usd)} left`}>
              <span className="grow-x bg-lime" style={{ width: `${pctOf(cap.data.committed_usd, cap.data.cap_usd)}%` }} />
              <span className="grow-x bg-amber" style={{ width: `${pctOf(cap.data.reserved_usd, cap.data.cap_usd)}%` }} />
            </span>
          </button>
        )}
      </PageHead>

      <ErrorNote error={error} />
      {loading && <Loading rows={3} />}
      {data && list.length === 0 && (
        <Panel tone="lime" className="flex flex-col items-start gap-2 p-8 sm:p-12">
          <Eyebrow>Queue clear</Eyebrow>
          <p className="font-display text-4xl font-semibold tracking-tight sm:text-6xl"><KineticText text="Nothing waiting." /></p>
          <p className="max-w-md text-sm opacity-75">No payouts are waiting for approval. Payouts under {usd(threshold)} go out on their own; anything bigger lands here.</p>
        </Panel>
      )}
      {list.length > 0 && (
        <p className="reveal mb-4 text-sm text-mute">
          <span className="num font-display text-2xl text-fog">{usd(total)}</span> across {list.length} payout{list.length > 1 ? "s" : ""}
        </p>
      )}
      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        {list.map((p, i) => (
          <Panel key={p.id} i={i} className="flex flex-col p-0">
            {/* Cheque header */}
            <div className="relative border-b border-dashed border-white/10 p-6">
              <span className="absolute -bottom-3 -left-3 rounded-full bg-ink size-6" />
              <span className="absolute -bottom-3 -right-3 rounded-full bg-ink size-6" />
              <div className="flex items-center justify-between gap-3">
                <Eyebrow>Held payout</Eyebrow>
                <span className="font-mono text-[11px] text-dim" title={when(p.created_at)}>{ago(p.created_at)}</span>
              </div>
              <p className="num mt-4 font-display text-5xl font-semibold tracking-tighter"><CountUp value={p.amount_usd} format={(v) => usd(v)} /></p>
              {nearOf(p.amount_usd, p.usd_per_near) && (
                <p className="mt-1 font-mono text-xs text-mute">
                  ≈ {nearOf(p.amount_usd, p.usd_per_near)} NEAR at {Number(p.usd_per_near)} USD/NEAR
                </p>
              )}
            </div>
            <div className="flex flex-1 flex-col p-6">
              <p className="font-mono text-[10px] uppercase tracking-widest text-dim">Pay to</p>
              {p.destination ? (
                <>
                  <p className="mt-1 text-sm"><Addr value={p.destination.address} /></p>
                  <p className="mt-1.5 flex flex-wrap items-center gap-1.5 text-xs text-mute">
                    <Badge tone="violet">{destLabel(p.destination)}</Badge> via NEAR Intents
                  </p>
                  <p className="mt-1 text-xs text-dim">Signed by NEAR <Addr value={p.recipient} /></p>
                </>
              ) : (
                <p className="mt-1 text-sm"><Addr value={p.recipient} /></p>
              )}
              {p.reason && (
                <>
                  <p className="mt-4 font-mono text-[10px] uppercase tracking-widest text-dim">Why it's held</p>
                  <p className="mt-1 text-sm text-mute">{p.reason}</p>
                </>
              )}
              <p className="mt-4 font-mono text-[10px] uppercase tracking-widest text-dim">Requested</p>
              <p className="mt-1 text-sm text-mute">{when(p.created_at)}</p>
              <div className="mt-auto grid grid-cols-2 gap-2 pt-6">
                <Button variant="ghost" disabled={busy === p.id} onClick={() => setRejecting(p)}>Reject</Button>
                <Button disabled={busy === p.id} onClick={() => setApproving(p)}>
                  <Icon name="check" size={16} /> Approve
                </Button>
              </div>
            </div>
          </Panel>
        ))}
      </div>
      <p className="mt-8 text-center text-xs text-dim">Approving never overrides the monthly cap. The signer re-checks every payout against its signed authorization, the worker's verified address, the tax form and the cap.</p>
      <CrossChain />
      <Approve key={`approve-${approving?.id ?? "none"}`} payout={approving} busy={!!approving && busy === approving.id} onClose={() => setApproving(null)} onConfirm={() => act(approving, "approve")} />
      <Reject key={`reject-${rejecting?.id ?? "none"}`} payout={rejecting} busy={!!rejecting && busy === rejecting.id} onClose={() => setRejecting(null)} onConfirm={(reason) => act(rejecting, "reject", { reason })} />
    </>
  );
}

// What exactly is about to happen: amount, the full recipient address
// (grouped in fours so it can be checked character by character), and why
// the payout was held.
function PayoutSummary({ p }) {
  // Cross-chain: the money lands at the destination address; that is what to check.
  const payTo = p.destination ? p.destination.address : p.recipient;
  const groups = String(payTo).match(/.{1,4}/g) || [];
  const long = String(payTo).length > 24;
  return (
    <div className="rounded-2xl bg-white/3 ring-1 ring-white/10">
      <div className="flex items-end justify-between gap-3 border-b border-white/5 p-4">
        <div>
          <p className="text-xs text-mute">Amount</p>
          <p className="num mt-0.5 font-display text-3xl font-semibold tracking-tight">{usd(p.amount_usd)}</p>
        </div>
        {nearOf(p.amount_usd, p.usd_per_near) && (
          <p className="num pb-1 text-right text-xs text-mute">
            ≈ {nearOf(p.amount_usd, p.usd_per_near)} NEAR
            <br />
            at {Number(p.usd_per_near)} USD/NEAR
          </p>
        )}
      </div>
      <dl className="space-y-3 p-4 text-sm">
        <div>
          <dt className="flex items-center justify-between text-xs text-mute">
            {p.destination ? `Recipient on ${p.destination.chain_name || p.destination.chain}` : "Recipient"}
            {long && <Addr value={payTo} className="text-[11px] text-dim" />}
          </dt>
          <dd className="mt-1 font-mono text-[13px] leading-relaxed text-fog">
            {long ? groups.map((g, i) => <span key={i} className={i % 2 ? "text-fog/70" : ""}>{g} </span>) : payTo}
          </dd>
        </div>
        {p.destination && (
          <div>
            <dt className="text-xs text-mute">Route</dt>
            <dd className="mt-1 text-fog/90">
              wNEAR from the treasury → {destLabel(p.destination)}, delivered by NEAR Intents solvers.
              <span className="mt-0.5 block text-xs text-mute">The worker signed this destination with their verified NEAR account <span className="font-mono">{p.recipient}</span>.</span>
            </dd>
          </div>
        )}
        {p.reason && (
          <div>
            <dt className="text-xs text-mute">Why it's held</dt>
            <dd className="mt-1 text-fog/90">{p.reason}</dd>
          </div>
        )}
        <div className="flex justify-between gap-3 text-xs">
          <dt className="text-mute">Requested</dt>
          <dd className="text-right text-fog/80">{ago(p.created_at)} · {when(p.created_at)}</dd>
        </div>
      </dl>
    </div>
  );
}

function Approve({ payout, busy, onClose, onConfirm }) {
  const [checked, setChecked] = useState(false);
  if (!payout) return null;
  return (
    <Modal open onClose={onClose} kicker="Confirm approval" title={`Approve ${usd(payout.amount_usd)}?`}>
      <div className="space-y-4">
        <PayoutSummary p={payout} />
        <label className="flex cursor-pointer items-start gap-3 rounded-2xl bg-white/3 p-4 text-sm ring-1 ring-white/10 transition hover:bg-white/5">
          <input type="checkbox" checked={checked} onChange={(e) => setChecked(e.target.checked)} className="mt-0.5 size-4 accent-lime" autoFocus />
          <span>
            I've checked the recipient address and the amount.
            <span className="mt-0.5 block text-xs text-mute">
              {payout.destination
                ? `Once delivered on ${payout.destination.chain_name || payout.destination.chain}, a payout can't be reversed. If NEAR Intents can't fill it, the deposit is refunded to the treasury.`
                : "Once the signer sends it on NEAR, a payout can't be reversed."}
            </span>
          </span>
        </label>
        <div className="grid grid-cols-2 gap-3">
          <Button type="button" variant="ghost" onClick={onClose}>Cancel</Button>
          <Button type="button" disabled={!checked || busy} onClick={onConfirm}>
            <Icon name="check" size={16} /> {busy ? "Approving…" : "Approve"}
          </Button>
        </div>
        <p className="text-center text-xs text-dim">Approving never overrides the monthly cap. The signer re-checks the signed approval, the worker's verified address, the tax form and the cap before sending.</p>
      </div>
    </Modal>
  );
}

const REASONS = ["Wrong recipient", "Amount looks wrong", "Duplicate payout", "Work not accepted"];

function Reject({ payout, busy, onClose, onConfirm }) {
  const [reason, setReason] = useState("");
  if (!payout) return null;
  const submit = (e) => {
    e.preventDefault();
    if (reason.trim()) onConfirm(reason);
  };
  return (
    <Modal open onClose={onClose} kicker="Confirm rejection" title={`Reject ${usd(payout.amount_usd)}?`}>
      <form onSubmit={submit} className="space-y-4">
        <PayoutSummary p={payout} />
        <div>
          <Textarea label="Reason (kept in the audit log)" value={reason} onChange={setReason} rows={3} required autoFocus />
          <div className="mt-2 flex flex-wrap gap-1.5" aria-label="Common reasons">
            {REASONS.map((r) => (
              <button
                key={r}
                type="button"
                onClick={() => setReason(r)}
                aria-pressed={reason === r}
                className={`rounded-full px-3 py-1 text-xs transition ${reason === r ? "bg-ember text-ink" : "bg-white/5 text-mute hover:bg-white/10 hover:text-fog"}`}
              >
                {r}
              </button>
            ))}
          </div>
        </div>
        <div className="grid grid-cols-2 gap-3">
          <Button type="button" variant="ghost" onClick={onClose}>Cancel</Button>
          <Button type="submit" variant="danger" disabled={busy || !reason.trim()}>{busy ? "Rejecting…" : "Reject"}</Button>
        </div>
        <p className="text-center text-xs text-dim">Nothing is sent. The reserved amount goes back to this month's cap.</p>
      </form>
    </Modal>
  );
}

// Recent payouts delivered on other chains through NEAR Intents: the NEAR
// deposit transaction, the intent's status, and the destination-chain fill.
function CrossChain() {
  const { data, error } = useLoad("/payouts/cross-chain", { poll: 15000 });
  const list = data?.payouts || [];
  if (error) return <div className="mt-10"><ErrorNote error={error} /></div>;
  if (!data || list.length === 0) return null;
  return (
    <Panel className="mt-10 p-6">
      <PanelHead eyebrow="NEAR Intents" title="Cross-chain payouts" action={<Badge tone="violet">{list.length}</Badge>} />
      <p className="-mt-2 mb-4 text-xs text-mute">The treasury stays on NEAR. Workers who signed a destination with their NEAR key are paid on that chain: the signer deposits wNEAR into a checked 1Click quote and solvers deliver.</p>
      <ul className="divide-y divide-white/5">
        {list.map((x) => (
          <li key={x.id} className="grid gap-2 py-3 text-sm sm:grid-cols-[7rem_1fr_auto] sm:items-center">
            <span className="num font-display text-lg">{usd(x.amount_usd)}</span>
            <div className="min-w-0">
              <p className="flex flex-wrap items-center gap-2">
                <Badge tone="violet">{destLabel(x.destination)}</Badge>
                <Addr value={x.destination.address} className="text-xs text-fog/80" />
              </p>
              <p className="mt-1 flex flex-wrap gap-x-3 gap-y-1 font-mono text-[11px] text-dim">
                {x.near_tx_hash && (
                  <a className="underline decoration-dotted hover:text-fog" href={nearblocks(data.network, x.near_tx_hash)} target="_blank" rel="noreferrer">NEAR deposit ↗</a>
                )}
                {(x.destination_txs || []).map((t) =>
                  t.explorer_url ? (
                    <a key={t.hash} className="underline decoration-dotted hover:text-fog" href={t.explorer_url} target="_blank" rel="noreferrer">{x.destination.chain_name} tx ↗</a>
                  ) : (
                    <span key={t.hash} title={t.hash}>{x.destination.chain_name} fill: {t.hash.startsWith("simulated") ? "simulated (testnet)" : t.hash.slice(0, 10)}</span>
                  ),
                )}
                {x.amount_out && <span>quoted out {x.amount_out} {assetLabel(x.destination.asset_id)}</span>}
              </p>
            </div>
            <div className="flex items-center gap-2 sm:justify-end">
              {x.intent_status && <span className="font-mono text-[10px] text-dim">{x.intent_status}</span>}
              <StatusBadge status={x.status} />
            </div>
          </li>
        ))}
      </ul>
    </Panel>
  );
}
