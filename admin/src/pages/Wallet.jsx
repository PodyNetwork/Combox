// The company wallet: the NEAR treasury that pays workers. Its account,
// network and balance, where to deposit (address and QR code), and every
// transaction, paged and filtered by the API. Read-only: nothing here moves
// funds; payouts are signed by the signer alone, within the monthly cap.
import { Badge, CopyBlock, ErrorNote, Eyebrow, Icon, Loading, PageHead, Panel, PanelHead, ago, useApp, useLoad } from "../ui.jsx";
import { NET, NotConnected, dollars, near } from "../components/Treasury.jsx";
import { Qr } from "../components/Qr.jsx";
import { TxHistory } from "../components/TxHistory.jsx";

const MANAGED = { signer: "Payouts signer (OpenBao key)", platform: "Combox platform", none: "Not set up" };

export default function Wallet() {
  const { platform: isPlatform, navigate } = useApp();
  const { data, error } = useLoad("/admin/treasury", { poll: 60000 });
  const net = NET[data?.network] ?? NET.testnet;
  return (
    <>
      <PageHead kicker={`Company wallet${data ? ` · ${net.label}` : ""}`} title="Wallet">
        {data && <Badge tone={net.tone} dot>{net.label}</Badge>}
      </PageHead>
      <ErrorNote error={error} />
      {!data && !error && <Loading rows={6} />}
      {data && !data.account && (
        <Panel className="p-6">
          <PanelHead eyebrow="Company wallet" title="NEAR treasury" />
          <NotConnected data={data} isPlatform={isPlatform} onSettings={() => navigate("settings")} />
        </Panel>
      )}
      {data?.account && (
        <div className="space-y-4">
          <div className="grid gap-4 lg:grid-cols-5">
            <Details data={data} className="lg:col-span-3" />
            <Deposit data={data} className="lg:col-span-2" />
          </div>
          <TxHistory scope="wallet" i={2} />
        </div>
      )}
    </>
  );
}

function Details({ data, className }) {
  const b = data.balance;
  const price = data.price;
  return (
    <Panel className={`flex flex-col p-6 sm:p-8 ${className}`}>
      <PanelHead eyebrow="NEAR treasury" title="Balance" />
      {b?.usd != null ? (
        <>
          <p className="num font-display text-5xl font-semibold leading-none tracking-tighter sm:text-6xl">{dollars(b.usd)}</p>
          <p className="num mt-2 text-sm text-mute" title={`${b.near} NEAR`}>{near(b.near)} NEAR</p>
        </>
      ) : (
        <p className="num font-display text-5xl font-semibold leading-none tracking-tighter sm:text-6xl" title={b ? `${b.near} NEAR` : undefined}>
          {b ? near(b.near) : "—"}
          <span className="ml-2 font-sans text-base font-normal tracking-normal text-mute">NEAR</span>
        </p>
      )}
      {b && !b.on_chain && <p className="mt-2 text-xs text-mute">Empty. The account appears on NEAR with its first deposit.</p>}
      {data.error && <p className="mt-2 text-xs text-amber first-letter:uppercase">{data.error}</p>}

      <div className="mt-6 grid grid-cols-2 gap-2 sm:grid-cols-3">
        <Stat label="Available" value={b ? `${near(b.available_near)} NEAR` : "—"} note="after storage" />
        <Stat label="Staked" value={b ? `${near(b.locked_near)} NEAR` : "—"} />
        <Stat
          label="NEAR price"
          value={price ? dollars(price.usd_per_near) : "—"}
          note={price ? (price.pinned ? "fixed testnet rate" : `as of ${ago(price.as_of)}`) : "no price right now"}
        />
      </div>

      <dl className="mt-6 divide-y divide-white/7 text-sm">
        <Line label="Account">
          <code className="break-all font-mono text-xs text-fog">{data.account}</code>
        </Line>
        <Line label="Network">{data.network}</Line>
        <Line label="Key held by">{MANAGED[data.managed_by] || data.managed_by}</Line>
        {b?.as_of && <Line label="Balance read">{ago(b.as_of)}</Line>}
      </dl>
      {data.explorer_url && (
        <a href={data.explorer_url} target="_blank" rel="noreferrer noopener" className="mt-auto inline-flex items-center gap-1 self-start pt-5 text-xs text-mute transition hover:text-fog">
          View on NearBlocks <Icon name="external" size={12} />
        </a>
      )}
    </Panel>
  );
}

function Stat({ label, value, note }) {
  return (
    <div className="min-w-0 rounded-xl bg-white/4 px-3 py-2.5">
      <p className="text-[11px] text-mute">{label}</p>
      <p className="num mt-0.5 truncate text-sm font-medium" title={value}>{value}</p>
      {note && <p className="mt-0.5 truncate text-[11px] text-dim">{note}</p>}
    </div>
  );
}

function Line({ label, children }) {
  return (
    <div className="flex items-baseline gap-4 py-2.5">
      <dt className="w-24 shrink-0 text-mute">{label}</dt>
      <dd className="min-w-0 flex-1 text-right sm:text-left">{children}</dd>
    </div>
  );
}

function Deposit({ data, className }) {
  const mainnet = data.network === "mainnet";
  const net = NET[data.network] ?? NET.testnet;
  return (
    <Panel i={1} className={`flex flex-col p-6 ${className}`}>
      <PanelHead eyebrow="Deposit" title="Send NEAR to the treasury" />
      <div className="flex justify-center">
        <Qr value={data.account} size={196} label={`QR code of the treasury account on ${net.label}`} className="p-0 ring-1 ring-white/10" />
      </div>
      <p className={`mx-auto mt-4 max-w-xs text-center text-xs ${mainnet ? "text-lime" : "text-amber"}`}>
        {mainnet ? "Main network: this is real NEAR." : "Test network: send testnet NEAR only. It has no real value."}
      </p>
      <div className="mt-4">
        <CopyBlock label={`Treasury account · ${net.label}`} value={data.account} />
      </div>
      <ul className="mt-4 space-y-2 text-xs text-mute">
        {[
          "Native NEAR only. Tokens such as USDC sent here can't pay workers.",
          `Send on ${data.network}. NEAR sent on ${mainnet ? "testnet" : "mainnet"} won't show up here.`,
          "Deposits appear in the history below within a minute.",
        ].map((t) => (
          <li key={t} className="flex items-start gap-2">
            <Icon name="check" size={14} className="mt-0.5 shrink-0 text-lime" />
            <span>{t}</span>
          </li>
        ))}
      </ul>
      <Eyebrow className="mt-auto pt-5 text-dim">Nothing is sent from this screen</Eyebrow>
    </Panel>
  );
}
