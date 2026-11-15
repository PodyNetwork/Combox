// Treasury yield on Rhea lending, in full: where the treasury's NEAR is, why
// the signer keeps that much liquid (the floor, worked out step by step),
// what it does next, the rules it follows, and every move it made.
import { useEffect, useState } from "react";
import { Addr, Badge, CountUp, Empty, ErrorNote, Icon, Loading, PageHead, Panel, PanelHead, ago, human, pctOf, useApp, usd, when } from "../ui.jsx";
import { StateDot, YieldSwitch, balancesOf, fmt, nearOf, pct, stateOf, useTreasuryYield } from "../components/TreasuryYield.jsx";

const MOVE_TONE = { succeeded: "lime", submitted: "sky", failed: "ember" };
const KIND = { register: "Registered on Rhea", supply: "Supplied", withdraw: "Withdrew", unwrap: "Unwrapped" };
const KIND_ICON = { register: "key", supply: "arrow", withdraw: "out", unwrap: "check" };
const CHECK_EVERY_SECS = 60;

function useNow(ms = 1000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [ms]);
  return now;
}

export default function TreasuryYieldPage() {
  const y = useTreasuryYield({ poll: 15000 });
  const { data, error } = y;
  const state = stateOf(data);
  const network = data?.network;
  return (
    <>
      <PageHead kicker={`Rhea Finance lending${network ? ` · ${network}` : ""}`} title="Treasury yield">
        {data && (
          <div className="flex items-center gap-3 rounded-full bg-white/5 py-1.5 pl-4 pr-2 ring-1 ring-inset ring-white/10">
            {state && <Badge tone={state.tone} dot={state.tone === "sky"}>{state.label}</Badge>}
            <YieldSwitch y={y} small={false} />
          </div>
        )}
      </PageHead>
      <ErrorNote error={error} />
      {!data && !error && <Loading rows={6} />}
      {data && (
        <div className="space-y-4">
          <Position data={data} state={state} />
          <div className="grid gap-4 lg:grid-cols-5">
            <Floor data={data} className="lg:col-span-3" />
            <NextCheck data={data} state={state} className="lg:col-span-2" />
          </div>
          <Moves data={data} />
          <Footer data={data} />
        </div>
      )}
    </>
  );
}

// ------------------------------------------------------------- position

function Position({ data, state }) {
  const plan = data.plan;
  const b = balancesOf(plan);
  const mainnet = data.network === "mainnet";
  const earned = Number(data.earned_near ?? 0);
  // Liquid first (what payroll draws on), then wNEAR on its way back, then Rhea.
  const segments = [
    { key: "liquid", label: "Liquid NEAR", value: b.liquid, color: "bg-fog/70" },
    { key: "wnear", label: "wNEAR, unwrapping", value: b.wnear, color: "bg-sky" },
    { key: "rhea", label: "Earning in Rhea", value: b.supplied, color: "bg-lime" },
  ];
  const markers = [
    b.floor != null && { key: "floor", label: "Floor", at: b.floor },
    b.target != null && { key: "target", label: "Target", at: b.target },
  ].filter(Boolean);
  const floorAbove = b.floor != null && b.floor > b.total;
  return (
    <Panel className="p-6 sm:p-8">
      <div className="grid gap-6 sm:grid-cols-3">
        <Stat label="Earning in Rhea" value={b.supplied} big accent>
          <span className="num">{pct(b.apr)} APR</span>
          {!mainnet && <span className="ml-1.5 rounded bg-white/5 px-1.5 py-0.5 font-mono text-[10px] text-dim" title="Testnet lending rates are not meaningful; mainnet wNEAR earns well under 1%.">testnet rate</span>}
          {earned > 0 && <span className="block">earned so far ≈ <span className="num">{fmt(earned, 6)}</span> NEAR</span>}
        </Stat>
        <Stat label="Liquid for payroll" value={b.liquid + b.wnear}>
          {b.floor != null ? <>floor <span className="num">{fmt(b.floor, 3)}</span> · target <span className="num">{fmt(b.target, 3)}</span> NEAR</> : "no fresh price: floor covers owed payouts only"}
          {b.wnear > 0 && <span className="block text-sky">incl. {fmt(b.wnear, 3)} wNEAR being unwrapped</span>}
        </Stat>
        <Stat label="Treasury total" value={b.total}>
          {b.total > 0 ? <><span className="num">{Math.round(pctOf(b.supplied, b.total))}%</span> in Rhea · at most {data.rules.max_deployed_pct}%</> : "empty"}
        </Stat>
      </div>

      {plan && b.total > 0 && (
        <div className="relative mt-12">
          {markers.map((m) => {
            const left = Math.min(100, pctOf(m.at, b.total));
            return (
              <div key={m.key} className="absolute -top-6 bottom-0 z-10" style={{ left: `${left}%` }} aria-hidden="true">
                <span className={`absolute -top-0.5 whitespace-nowrap font-mono text-[10px] uppercase tracking-wider text-fog ${left > 85 ? "right-1" : left < 8 ? "left-1" : "-translate-x-1/2"} ${m.key === "target" && markers.length > 1 && Math.abs(pctOf(markers[0].at - m.at, b.total)) < 10 ? "-top-4" : ""}`}>
                  {m.label}
                </span>
                <span className={`absolute top-4 bottom-0 w-px -translate-x-1/2 ${m.key === "floor" ? "bg-fog" : "bg-fog/40"}`} />
              </div>
            );
          })}
          <div className="flex h-12 w-full overflow-hidden rounded-2xl bg-white/6 ring-1 ring-inset ring-white/10" role="img" aria-label={segments.map((s) => `${s.label} ${fmt(s.value, 3)} NEAR`).join(", ")}>
            {segments.map((s, i) =>
              s.value > 0 ? <div key={s.key} className={`grow-x ${s.color} h-full border-r-2 border-ink last:border-r-0`} style={{ width: `${pctOf(s.value, b.total)}%`, "--i": i }} title={`${s.label}: ${fmt(s.value, 4)} NEAR`} /> : null,
            )}
          </div>
          <div className="mt-3 flex flex-wrap gap-x-5 gap-y-1 text-xs text-mute">
            {segments.filter((s) => s.value > 0).map((s) => (
              <span key={s.key} className="inline-flex items-center gap-1.5"><span className={`rounded-sm size-2.5 ${s.color}`} />{s.label} <span className="num text-fog">{fmt(s.value, 3)}</span></span>
            ))}
          </div>
          {floorAbove && (
            <p className="mt-3 rounded-xl bg-amber/10 px-3 py-2 text-xs text-amber ring-1 ring-inset ring-amber/30">
              The floor ({fmt(b.floor, 3)} NEAR) is more than the whole treasury, so nothing can be supplied. {state?.label === "Needs funding" && "Add NEAR or lower the monthly cap."}
            </p>
          )}
        </div>
      )}
    </Panel>
  );
}

function Stat({ label, value, big, accent, children }) {
  return (
    <div>
      <p className="text-sm text-mute">{label}</p>
      <p className={`num mt-1 font-display font-semibold leading-none tracking-tight ${big ? "text-5xl sm:text-6xl" : "text-4xl"} ${accent ? "text-lime" : ""}`}>
        <CountUp value={value} format={(v) => fmt(v, v >= 100 ? 1 : 3)} />
        <span className="ml-1.5 font-sans text-base font-normal text-mute">NEAR</span>
      </p>
      <p className="mt-2 text-xs leading-relaxed text-mute">{children}</p>
    </div>
  );
}

// ---------------------------------------------------------------- floor

function Floor({ data, className }) {
  const { navigate } = useApp();
  const plan = data.plan;
  const rules = data.rules;
  if (!plan) {
    return (
      <Panel i={1} className={`p-6 ${className}`}>
        <PanelHead eyebrow="How much stays liquid" title="The payroll floor" />
        <p className="text-sm text-mute">Shown after the signer's first check.</p>
      </Panel>
    );
  }
  const b = balancesOf(plan);
  const owed = nearOf(plan.obligations?.owed_yocto) ?? 0;
  const headroomUsd = Number(plan.obligations?.cap_headroom_usd ?? 0);
  const price = plan.usd_per_near != null ? Number(plan.usd_per_near) : null;
  const minLiquid = Number(rules.min_liquid_near);
  // Older plans don't carry the price; the floor still pins the cap's share.
  const headroomNear = price ? headroomUsd / price : b.floor != null ? Math.max(0, b.floor - owed - minLiquid) : null;
  const liquidNow = b.liquid + b.wnear;
  const room = b.target != null ? liquidNow - b.target : null;
  return (
    <Panel i={1} className={`p-6 ${className}`}>
      <PanelHead
        eyebrow="How much stays liquid"
        title="The payroll floor"
        action={<button type="button" onClick={() => navigate("spending")} className="inline-flex items-center gap-1 text-xs text-mute hover:text-fog">Monthly cap <Icon name="arrow" size={12} /></button>}
      />
      <p className="mb-3 text-xs text-mute">Everything payroll could still pay this month stays liquid. Only NEAR above the target goes to Rhea.</p>
      <ul className="divide-y divide-white/7 text-sm">
        <Line label="Payouts owed" note="waiting for approval, approved or retrying" value={owed} />
        <Line
          label="Monthly cap still unspent"
          note={price ? `${usd(headroomUsd)} at ${usd(price)} per NEAR` : `${usd(headroomUsd)} left this month`}
          value={headroomNear}
          missing="needs a fresh price"
        />
        <Line label="Always liquid" note="min_liquid_near" value={minLiquid} />
        <Line label="Floor" value={b.floor} strong missing="unknown without a price" />
        <Line label="Target" note={`floor + ${rules.liquidity_margin_pct}% margin, where supplying stops`} value={b.target} strong missing="—" />
      </ul>
      <div className="mt-4 flex flex-wrap items-baseline gap-x-2 rounded-xl bg-white/4 px-4 py-3 text-sm">
        <span className="text-mute">Liquid now</span>
        <span className="num font-medium">{fmt(liquidNow, 3)} NEAR</span>
        {room != null && (
          <span className={`ml-auto text-xs ${b.floor != null && liquidNow < b.floor ? "text-amber" : room > 0 ? "text-lime" : "text-mute"}`}>
            {b.floor != null && liquidNow < b.floor
              ? `${fmt(b.floor - liquidNow, 3)} NEAR below the floor`
              : room > 0
                ? `${fmt(room, 3)} NEAR above the target`
                : `${fmt(-room, 3)} NEAR under the target, above the floor`}
          </span>
        )}
      </div>
    </Panel>
  );
}

function Line({ label, note, value, strong, missing = "—" }) {
  return (
    <li className="flex items-baseline gap-4 py-2.5">
      <span className="min-w-0 flex-1">
        <span className={strong ? "font-semibold" : "font-medium"}>{label}</span>
        {note && <span className="block text-xs text-mute">{note}</span>}
      </span>
      <span className={`num text-right ${strong ? "font-semibold text-fog" : ""}`}>{value == null ? <span className="text-xs text-dim">{missing}</span> : <>{fmt(value, 3)} <span className="text-xs text-mute">NEAR</span></>}</span>
    </li>
  );
}

// ----------------------------------------------------------- next check

function NextCheck({ data, state, className }) {
  const now = useNow();
  const rules = data.rules;
  const checked = data.checked_at ? new Date(data.checked_at).getTime() : null;
  const next = checked ? Math.max(0, Math.round((checked + CHECK_EVERY_SECS * 1000 - now) / 1000)) : null;
  return (
    <Panel i={2} className={`flex flex-col p-6 ${className}`}>
      <PanelHead eyebrow="The signer" title="What happens next" />
      {state && (
        <div className="flex items-start gap-2.5">
          <span className="mt-1.5"><StateDot tone={state.tone} /></span>
          <p className="text-sm leading-relaxed"><span className="font-medium">{state.label}.</span> <span className="text-mute">{state.text}</span></p>
        </div>
      )}
      <p className="mt-3 font-mono text-[11px] text-dim">
        {checked ? <>checked {ago(data.checked_at)} · {next > 0 ? `next check in ~${next}s` : "next check due"}</> : "not checked yet"}
      </p>

      <div className="mt-5 grid grid-cols-2 gap-2">
        <Rule label="Margin above floor" value={`${rules.liquidity_margin_pct}%`} />
        <Rule label="Most in Rhea" value={`${rules.max_deployed_pct}%`} />
        <Rule label="Smallest move" value={`${rules.min_move_near} NEAR`} />
        <Rule label="Always liquid" value={`${rules.min_liquid_near} NEAR`} />
      </div>
      <p className="mt-2 text-[11px] text-dim">Rules live in <code className="font-mono">config/policy.toml</code> under <code className="font-mono">[treasury]</code>.</p>

      <div className="mt-auto pt-5 text-xs leading-relaxed text-mute">
        <p className="font-medium text-fog">Getting NEAR back</p>
        <p className="mt-1">
          Turn the switch off and the signer withdraws everything from Rhea, with interest, then unwraps it to liquid NEAR. It also pulls back on its own whenever liquid NEAR drops below the floor.
        </p>
      </div>
    </Panel>
  );
}

function Rule({ label, value }) {
  return (
    <div className="rounded-xl bg-white/4 px-3 py-2.5">
      <p className="text-[11px] text-mute">{label}</p>
      <p className="num mt-0.5 text-sm font-medium">{value}</p>
    </div>
  );
}

// ---------------------------------------------------------------- moves

function Moves({ data }) {
  const moves = data.moves || [];
  return (
    <Panel i={3} className="p-2 sm:p-3">
      <PanelHead className="px-4 pt-4" eyebrow="On chain" title="Moves" action={<span className="pr-1 text-xs text-mute">last {moves.length || 0}</span>} />
      {moves.length === 0 ? (
        <div className="p-4"><Empty title="No moves yet" text="Registering, supplying, withdrawing and unwrapping each show up here with a NearBlocks link." /></div>
      ) : (
        <ul className="divide-y divide-white/7">
          {moves.map((m) => (
            <li key={m.id} className="flex items-start gap-4 px-4 py-3.5">
              <span className="mt-0.5 grid shrink-0 place-items-center rounded-xl bg-white/5 size-9 text-mute">
                <Icon name={KIND_ICON[m.kind] || "bolt"} size={16} className={m.kind === "withdraw" ? "" : m.kind === "supply" ? "-rotate-45" : ""} />
              </span>
              <span className="min-w-0 flex-1">
                <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                  <span className="text-sm font-medium">{KIND[m.kind] || human(m.kind)} <span className="num">{fmt(m.amount_near, 4)}</span> NEAR</span>
                  <Badge tone={MOVE_TONE[m.status] || "mute"} dot={m.status === "submitted"}>{human(m.status)}</Badge>
                </span>
                <span className="mt-0.5 block text-xs text-mute">{m.reason}</span>
                {m.error && <span className="mt-1 block text-xs text-ember">{m.error}</span>}
              </span>
              <a href={m.explorer_url} target="_blank" rel="noreferrer noopener" className="shrink-0 text-right text-xs text-mute hover:text-fog" title={`${when(m.created_at)} · ${m.tx_hash}`}>
                <span className="inline-flex items-center gap-1">{ago(m.created_at)} <Icon name="external" size={12} /></span>
                <span className="mt-0.5 hidden font-mono text-[10px] text-dim sm:block">{m.tx_hash?.slice(0, 8)}…</span>
              </a>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

// --------------------------------------------------------------- footer

function Footer({ data }) {
  const base = data.network === "mainnet" ? "https://nearblocks.io" : "https://testnet.nearblocks.io";
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-1 text-xs text-mute">
      <span className="inline-flex items-center gap-1.5"><Icon name="shield" size={14} className="text-lime" /> Supply only · never borrows, never collateral · funds only return to the treasury</span>
      {data.lending_contract && (
        <span className="inline-flex min-w-0 items-center gap-1 sm:ml-auto">
          Lending contract <Addr value={data.lending_contract} className="text-fog" />
          <a href={`${base}/address/${data.lending_contract}`} target="_blank" rel="noreferrer noopener" className="text-mute hover:text-fog" aria-label="Open the lending contract on NearBlocks"><Icon name="external" size={12} /></a>
        </span>
      )}
    </div>
  );
}
