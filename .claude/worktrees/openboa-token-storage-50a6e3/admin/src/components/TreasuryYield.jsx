// Treasury yield on Rhea lending: how much of the treasury is earning, how
// much stays liquid for payroll and why, and every move the signer made.
// Read-only: the signer decides and moves by the policy's rules alone. It is
// turned on or off in Settings.
import { Badge, ErrorNote, Icon, Loading, Panel, PanelHead, ago, human, useApp, useLoad } from "../ui.jsx";

// yocto string -> NEAR number (display only).
const nearOf = (y) => (y == null ? null : Number(BigInt(y) / 10n ** 20n) / 1e4);
const fmt = (n, d = 2) => (n == null ? "—" : Number(n).toLocaleString(undefined, { maximumFractionDigits: d }));
const pct = (apr) => (apr == null ? "—" : `${(Number(apr) * 100).toLocaleString(undefined, { maximumFractionDigits: 2 })}%`);
const MOVE_TONE = { succeeded: "lime", submitted: "sky", failed: "ember" };
const KIND = { register: "Registered on Rhea", supply: "Supplied", withdraw: "Withdrew", unwrap: "Unwrapped" };

export function TreasuryYieldCard({ i = 0, className = "" }) {
  const { navigate } = useApp();
  const { data, error } = useLoad("/admin/treasury/yield", { poll: 60000 });
  const plan = data?.plan;
  const pos = plan?.position;
  const liquid = nearOf(pos?.liquid_yocto);
  const supplied = nearOf(pos?.supplied_yocto);
  const floor = nearOf(plan?.floor_yocto);
  const total = (liquid ?? 0) + (supplied ?? 0);
  const share = total > 0 ? Math.round(((supplied ?? 0) / total) * 100) : 0;
  return (
    <Panel i={i} className={`flex flex-col p-6 ${className}`}>
      <PanelHead
        eyebrow="Rhea Finance lending"
        title="Treasury yield"
        action={data && <Badge tone={data.enabled ? "lime" : "mute"} dot={data.enabled}>{data.enabled ? "On" : "Off"}</Badge>}
      />
      <ErrorNote error={error} />
      {!data && !error && <Loading rows={2} />}
      {data && !plan && (
        <p className="text-sm text-mute">
          {data.enabled
            ? "Waiting for the signer's first check of the treasury."
            : "Off. With treasury yield on, NEAR the monthly cap can't spend this month is supplied to Rhea lending and pulled back before payroll needs it."}
        </p>
      )}
      {plan && (
        <>
          <div className="grid grid-cols-2 gap-4">
            <div>
              <p className="text-xs text-mute">Earning in Rhea</p>
              <p className="num mt-1 font-display text-3xl font-semibold tracking-tight">{fmt(supplied)}<span className="ml-1.5 font-sans text-sm font-normal text-mute">NEAR</span></p>
              <p className="num mt-1 text-xs text-mute">{pct(pos.supply_apr)} APR · {share}% of treasury</p>
            </div>
            <div>
              <p className="text-xs text-mute">Liquid for payroll</p>
              <p className="num mt-1 font-display text-3xl font-semibold tracking-tight">{fmt(liquid)}<span className="ml-1.5 font-sans text-sm font-normal text-mute">NEAR</span></p>
              <p className="num mt-1 text-xs text-mute">floor {fmt(floor)} NEAR</p>
            </div>
          </div>
          {total > 0 && (
            <div className="mt-4 flex h-1.5 overflow-hidden rounded-full bg-white/5" aria-hidden="true">
              <div className="bg-lime" style={{ width: `${share}%` }} />
            </div>
          )}
          <p className="mt-4 text-xs leading-relaxed text-mute">
            {plan.reason}
            {data.earned_near != null && Number(data.earned_near) > 0 && <> · earned so far ≈ {fmt(data.earned_near, 4)} NEAR</>}
          </p>
          <p className="mt-1 text-[11px] text-dim">
            The floor keeps liquid what payroll may still pay this month: payouts owed plus the monthly cap's headroom, plus {data.rules.min_liquid_near} NEAR.
            Checked {ago(data.checked_at)}.
          </p>
        </>
      )}
      {data?.moves?.length > 0 && (
        <ul className="mt-4 divide-y divide-white/5 border-t border-white/5">
          {data.moves.slice(0, 5).map((m) => (
            <li key={m.id} className="flex items-center gap-3 py-2 text-xs">
              <Badge tone={MOVE_TONE[m.status] || "mute"}>{human(m.status)}</Badge>
              <span className="min-w-0 flex-1 truncate text-fog/90" title={m.error || m.reason}>
                {KIND[m.kind] || m.kind} <span className="num">{fmt(m.amount_near, 3)}</span> NEAR
              </span>
              <a href={m.explorer_url} target="_blank" rel="noreferrer noopener" className="inline-flex shrink-0 items-center gap-1 text-mute hover:text-fog" title={m.tx_hash}>
                {ago(m.created_at)} <Icon name="external" size={12} />
              </a>
            </li>
          ))}
        </ul>
      )}
      {data && (
        <div className="mt-auto flex flex-wrap items-center gap-x-3 gap-y-1 pt-4">
          <p className="mr-auto font-mono text-[10px] text-dim" title={data.lending_contract}>
            supply only · never borrows · funds only return to the treasury
          </p>
          <button type="button" onClick={() => navigate("settings")} className="inline-flex items-center gap-1 text-xs text-mute hover:text-fog">
            {data.enabled ? "Settings" : "Turn on in Settings"} <Icon name="arrow" size={12} />
          </button>
        </div>
      )}
    </Panel>
  );
}

