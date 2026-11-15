// The company's wallet: its NEAR treasury, which pays workers. Read-only:
// the account, its balance, and how to deposit. Nothing here moves funds;
// payouts are signed by the signer alone, within the monthly cap.
import { useEffect, useRef, useState } from "react";
import { Badge, Button, CopyBlock, ErrorNote, Eyebrow, Icon, Loading, Modal, Panel, PanelHead, useApp, useLoad, useToast } from "../ui.jsx";

export const NET = {
  mainnet: { label: "NEAR mainnet", tone: "lime" },
  testnet: { label: "NEAR testnet", tone: "amber" },
};
const LINK = "inline-flex items-center gap-1 text-xs text-mute transition hover:text-fog";

// "12.49818" -> "12.4982"; the exact value is in the title.
export const near = (s, digits = 4) => Number(s).toLocaleString(undefined, { maximumFractionDigits: digits });
export const dollars = (s) => Number(s).toLocaleString(undefined, { style: "currency", currency: "USD" });
// A 64-character implicit account reads better as its ends; names stay whole.
const shortAccount = (a) => (a.length > 24 ? `${a.slice(0, 6)}…${a.slice(-6)}` : a);

export function TreasuryCard({ i = 0, className = "" }) {
  const { platform: isPlatform, navigate } = useApp();
  const { data, error, reload } = useLoad("/admin/treasury", { poll: 60000 });
  const [depositing, setDepositing] = useState(false);
  // The dialog sits outside the panel: inside, the panel's stacking context
  // would let later cards paint over it.
  return (
    <>
      <TreasuryPanel
        i={i}
        className={className}
        data={data}
        error={error}
        isPlatform={isPlatform}
        onSettings={() => navigate("settings")}
        onDeposit={() => setDepositing(true)}
        onWallet={() => navigate("wallet")}
      />
      {depositing && data?.account && <DepositDialog treasury={data} onClose={() => setDepositing(false)} onCheck={reload} />}
    </>
  );
}

/** The card itself, from the /admin/treasury answer. */
export function TreasuryPanel({ i = 0, className = "", data, error, isPlatform, onSettings, onDeposit, onWallet }) {
  const net = NET[data?.network] ?? NET.testnet;
  const b = data?.balance;
  const usd = b?.usd;
  return (
    <Panel i={i} className={`flex flex-col p-6 ${className}`}>
      <PanelHead
        eyebrow="Company wallet"
        title="NEAR treasury"
        action={data && <Badge tone={net.tone} dot>{net.label}</Badge>}
      />
      <ErrorNote error={error} />
      {!data && !error && <Loading rows={2} />}
      {data && !data.account && <NotConnected data={data} isPlatform={isPlatform} onSettings={onSettings} />}
      {data?.account && (
        <>
          <p className="text-xs text-mute">Balance</p>
          {usd != null ? (
            <>
              <p className="num mt-1 font-display text-5xl font-semibold leading-none tracking-tighter">{dollars(usd)}</p>
              <p className="num mt-2 text-sm text-mute" title={`${b.near} NEAR`}>{near(b.near)} NEAR</p>
            </>
          ) : (
            <p className="num mt-1 font-display text-5xl font-semibold leading-none tracking-tighter" title={b ? `${b.near} NEAR` : undefined}>
              {b ? near(b.near) : "—"}
              <span className="ml-2 font-sans text-base font-normal tracking-normal text-mute">NEAR</span>
            </p>
          )}
          {b && !b.on_chain && <p className="mt-2 text-xs text-mute">Empty. The account appears on NEAR with its first deposit.</p>}
          {data.error && <p className="mt-2 text-xs text-amber first-letter:uppercase">{data.error}</p>}
          {onWallet && (
            <button type="button" onClick={onWallet} className={`${LINK} mt-4 self-start`}>
              Wallet and history <Icon name="arrow" size={12} />
            </button>
          )}
        </>
      )}
      {data && (
        <div className="mt-auto flex flex-wrap items-center justify-between gap-3 pt-6">
          <Button onClick={onDeposit} disabled={!data.account} className="shrink-0">
            <Icon name="plus" size={16} /> Deposit
          </Button>
          {data.account && <AccountChip account={data.account} explorer={data.explorer_url} />}
        </div>
      )}
    </Panel>
  );
}

/** The address, shortened, with copy and explorer buttons. */
function AccountChip({ account, explorer }) {
  const toast = useToast();
  const [copied, setCopied] = useState(false);
  const timer = useRef(null);
  useEffect(() => () => clearTimeout(timer.current), []);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(account);
      setCopied(true);
      clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopied(false), 1500);
    } catch {
      toast("Copy failed", "error");
    }
  };
  const btn = "grid size-7 place-items-center rounded-full text-mute transition hover:bg-white/10 hover:text-fog";
  return (
    <div className="flex min-w-0 items-center gap-1 rounded-full bg-black/40 py-1 pl-3 pr-1 ring-1 ring-white/10">
      <code className="min-w-0 truncate font-mono text-xs text-fog" title={account}>{shortAccount(account)}</code>
      <button type="button" onClick={copy} className={btn} aria-label="Copy treasury address" title="Copy address">
        <Icon name={copied ? "check" : "copy"} size={14} className={copied ? "text-lime" : ""} />
      </button>
      {explorer && (
        <a href={explorer} target="_blank" rel="noreferrer noopener" className={btn} aria-label="View on NearBlocks" title="View on NearBlocks">
          <Icon name="external" size={14} />
        </a>
      )}
    </div>
  );
}

export function NotConnected({ data, isPlatform, onSettings }) {
  if (data.error) return <p className="rounded-xl bg-amber/10 px-3 py-2 text-sm text-amber ring-1 ring-amber/30 first-letter:uppercase">{data.error}</p>;
  if (isPlatform)
    return (
      <div className="space-y-3">
        <p className="text-sm text-mute">No treasury yet. Create it in Settings, then deposit NEAR to start paying workers.</p>
        <button onClick={onSettings} className={LINK}>Open settings <Icon name="arrow" size={12} /></button>
      </div>
    );
  return <p className="text-sm text-mute">Not connected yet. The treasury address comes from the payouts signer: run ./scripts/start to set up payouts.</p>;
}

/** Where and how to deposit. Shows the address only; it never sends anything. */
export function DepositDialog({ treasury, onClose, onCheck }) {
  const { platform: isPlatform } = useApp();
  const [checking, setChecking] = useState(false);
  const mainnet = treasury.network === "mainnet";
  const net = NET[treasury.network] ?? NET.testnet;
  const b = treasury.balance;
  const check = async () => {
    setChecking(true);
    await onCheck();
    // The balance updates under the dialog; keep the button busy briefly.
    setTimeout(() => setChecking(false), 800);
  };
  return (
    <Modal open onClose={onClose} kicker="Company wallet" title="Deposit NEAR">
      <div className="space-y-4">
        <div className={`flex items-start gap-3 rounded-2xl px-4 py-3 ring-1 ${mainnet ? "bg-lime/10 ring-lime/30" : "bg-amber/10 ring-amber/30"}`}>
          <Badge tone={net.tone} dot className="mt-0.5 shrink-0">{net.label}</Badge>
          <p className={`text-sm ${mainnet ? "text-lime" : "text-amber"}`}>
            {mainnet ? "Main network: this is real NEAR." : "Test network: send testnet NEAR only. It has no real value."}
          </p>
        </div>
        <p className="text-sm text-mute">
          Send NEAR to this account from any NEAR wallet on <b className="font-medium text-fog">{treasury.network}</b>. Payouts to workers are paid from it; keep about a month of payouts in it.
        </p>
        <CopyBlock label={`Treasury account · ${net.label}`} value={treasury.account} />
        <ul className="space-y-2 text-sm text-mute">
          {[
            "Native NEAR only. Tokens such as USDC sent here can't pay workers.",
            `Send on ${treasury.network}. NEAR sent on ${mainnet ? "testnet" : "mainnet"} won't show up here.`,
            "Nothing is sent from this screen. Payouts are signed separately, within the monthly cap.",
            // A new implicit account doesn't exist until its first deposit,
            // and faucets refuse accounts that don't exist yet.
            !mainnet && !b?.on_chain &&
              (isPlatform
                ? "Faucets say \"invalid account\" until the first deposit: send testnet NEAR from a testnet wallet."
                : "Faucets say \"invalid account\" until the first deposit: run ./scripts/fund-testnet, or send from a testnet wallet."),
          ].filter(Boolean).map((t) => (
            <li key={t} className="flex items-start gap-2.5">
              <Icon name="check" size={16} className="mt-0.5 shrink-0 text-lime" />
              <span>{t}</span>
            </li>
          ))}
        </ul>
        <div className="flex items-center justify-between gap-3 rounded-2xl bg-white/3 px-4 py-3 ring-1 ring-white/10">
          <div>
            <Eyebrow>Balance now</Eyebrow>
            <p className="num mt-1 font-display text-xl font-semibold" title={b ? `${b.near} NEAR` : undefined}>
              {b ? `${near(b.near)} NEAR` : "—"}
            </p>
            {b?.usd != null && <p className="num mt-0.5 text-sm text-mute">≈ {dollars(b.usd)}</p>}
          </div>
          <Button size="sm" variant="ghost" onClick={check} disabled={checking}>
            {checking ? "Checking…" : "Check balance"}
          </Button>
        </div>
      </div>
      <div className="mt-5 flex flex-wrap items-center justify-end gap-3">
        {treasury.explorer_url && (
          <a href={treasury.explorer_url} target="_blank" rel="noreferrer noopener" className={LINK}>
            View on NearBlocks <Icon name="external" size={12} />
          </a>
        )}
        <Button variant="ghost" onClick={onClose}>Done</Button>
      </div>
    </Modal>
  );
}
