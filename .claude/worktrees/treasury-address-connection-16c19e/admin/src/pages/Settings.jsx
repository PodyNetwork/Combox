// Company settings: the plan (if none yet), the NEAR treasury, links, and
// the company's own stricter rules. The kill switch has its own page.
import { useCallback, useEffect, useState } from "react";
import { platform } from "../api.js";
import PlanForm from "../components/PlanForm.jsx";
import { Badge, Button, CopyField, ErrorNote, Icon, Loading, PageHead, Panel, PanelHead, useApp, useCompanyApi, useLoad, usd, when } from "../ui.jsx";

export default function Settings() {
  const { platform: isPlatform, company } = useApp();
  return (
    <>
      <PageHead kicker={isPlatform && company ? `${company.company.name} · ${company.role}` : "Company rules"} title="Settings" />
      <div className="space-y-4">
        {isPlatform && <CompanyDetails />}
        <Rules />
        <KillPointer />
      </div>
    </>
  );
}

function CompanyDetails() {
  const { session, slug, notify, company, reloadMe } = useApp();
  const [info, setInfo] = useState(null);
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(false);
  const reload = useCallback(() => platform(session, `/companies/${slug}`).then(setInfo, (e) => setError(e.message)), [session, slug]);
  useEffect(() => {
    reload();
  }, [reload]);
  const owner = company?.role === "owner";
  const setupTreasury = async () => {
    setBusy(true);
    try {
      await platform(session, `/companies/${slug}/treasury`, { method: "POST" });
      notify("Treasury created. Fund it to start paying workers.");
      await reload();
      reloadMe();
    } catch (e) {
      notify(e.message, "error");
    }
    setBusy(false);
  };
  if (error) return <ErrorNote error={error} />;
  if (!info) return <Loading rows={4} />;
  const account = info.company.treasury_account;
  return (
    <>
      {!info.has_plan && (
        <Panel className="p-6 sm:p-8">
          <PanelHead eyebrow="First step" title="Set your company's goal" />
          <p className="-mt-2 mb-6 max-w-2xl text-sm text-mute">Combox builds departments, AI workers, and tasks from it, and opens hiring requests for work that needs people.</p>
          <PlanForm session={session} slug={slug} onSaved={(r) => { notify(`Created ${r.departments} departments and ${r.workers_created} AI workers.`); reload(); }} />
        </Panel>
      )}
      <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
        <Panel i={1} className="p-6">
          <PanelHead
            eyebrow="NEAR treasury"
            title={account ? "Ready to fund" : "Not set up"}
            action={account ? <Badge tone="lime" dot>Created</Badge> : <Badge tone="amber">Payouts wait</Badge>}
          />
          <p className="-mt-2 mb-5 text-sm text-mute">Your company's own wallet for paying workers. Its key stays in OpenBao and can't be exported; Combox never holds your funds.</p>
          {account ? (
            <div className="space-y-3">
              <p className="font-mono text-[10px] uppercase tracking-widest text-dim">Treasury account (fund this)</p>
              <CopyField value={account} />
              <p className="text-sm text-mute">Send NEAR here from your own wallet. Keep about one month of payouts in it; the monthly cap still applies.</p>
            </div>
          ) : (
            <div className="space-y-3">
              <p className="text-sm text-fog/90">No treasury yet. Payouts wait until you create and fund one.</p>
              {!info.treasury_available && (
                <p className="rounded-xl bg-amber/10 px-3 py-2 text-sm text-amber ring-1 ring-amber/30">Treasuries aren't available on this platform yet.</p>
              )}
              <div className="flex flex-wrap items-center gap-3 pt-1">
                <Button onClick={setupTreasury} disabled={busy || !owner || !info.treasury_available}>{busy ? "Creating…" : "Create treasury"}</Button>
                {!owner && <span className="text-xs text-mute">Only the owner can create the treasury.</span>}
              </div>
            </div>
          )}
        </Panel>
        <Panel i={2} className="p-6">
          <PanelHead eyebrow="Links" title="Share and connect" />
          <dl className="space-y-4">
            <LinkRow label="Public jobs board" value={info.urls.jobs_board} open />
            <LinkRow label="API for this company" value={info.urls.api} />
            <LinkRow label="MCP server (meeting tools)" value={info.urls.mcp} />
          </dl>
        </Panel>
      </div>
    </>
  );
}

function LinkRow({ label, value, open = false }) {
  return (
    <div>
      <dt className="mb-1.5 flex items-center justify-between font-mono text-[10px] uppercase tracking-widest text-dim">
        {label}
        {open && (
          <a href={value} target="_blank" rel="noreferrer noopener" className="flex items-center gap-1 normal-case tracking-normal text-mute hover:text-fog">
            Open <Icon name="external" size={12} />
          </a>
        )}
      </dt>
      <dd><CopyField value={value} /></dd>
    </div>
  );
}

// A number field with a unit before or after it ("$", "days").
function UnitInput({ label, value, onChange, prefix, suffix, hint, ...rest }) {
  return (
    <label className="block text-sm">
      <span className="text-mute">{label}</span>
      <span className="mt-1.5 flex items-center rounded-xl border border-white/10 bg-black/40 transition focus-within:border-lime/60 focus-within:ring-4 focus-within:ring-lime/10">
        {prefix && <span className="pl-3.5 font-mono text-dim">{prefix}</span>}
        <input type="number" value={value} onChange={(e) => onChange(e.target.value)} className="num min-w-0 flex-1 bg-transparent px-3 py-2.5 text-sm text-fog placeholder:text-dim focus:outline-none" {...rest} />
        {suffix && <span className="pr-3.5 text-dim">{suffix}</span>}
      </span>
      {hint && <span className="mt-1 block text-xs text-dim">{hint}</span>}
    </label>
  );
}

function Rules() {
  const { notify } = useApp();
  const call = useCompanyApi();
  const { data, error, reload } = useLoad("/admin/policy");
  const [threshold, setThreshold] = useState(null);
  const [retention, setRetention] = useState(null);
  const [busy, setBusy] = useState(false);
  const t = threshold ?? (data?.company.approval_threshold_usd ?? "");
  const r = retention ?? (data?.company.candidate_retention_days ?? "");
  const save = async (e) => {
    e.preventDefault();
    setBusy(true);
    try {
      await call("/admin/policy", { method: "POST", body: { approval_threshold_usd: t === "" ? null : String(t), candidate_retention_days: r === "" ? null : Number(r) } });
      notify("Rules saved. The change is in the audit log.");
      setThreshold(null);
      setRetention(null);
      reload();
    } catch (err) {
      notify(err.message, "error");
    }
    setBusy(false);
  };
  const plat = data?.platform;
  // Only stricter rules are accepted; say so before the API does.
  const looser = data && ((t !== "" && Number(t) > Number(plat.approval_threshold_usd)) || (r !== "" && Number(r) > Number(plat.candidate_retention_days)));
  return (
    <Panel i={3} className="p-6 sm:p-8">
      <PanelHead eyebrow="Stricter than the platform" title="Payout and privacy rules" />
      <p className="-mt-2 mb-6 max-w-2xl text-sm text-mute">Make the platform's rules stricter for this company. They can never be loosened here.</p>
      <ErrorNote error={error} />
      {!data && !error && <Loading rows={2} />}
      {data && (
        <form onSubmit={save} noValidate className="space-y-5">
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <Rule
              title="Payout approval"
              effective={usd(data.effective.approval_threshold_usd)}
              own={data.company.approval_threshold_usd != null}
            >
              <UnitInput
                label="Approval needed from"
                prefix="$"
                value={t}
                onChange={setThreshold}
                placeholder={String(Number(plat.approval_threshold_usd))}
                min={0.01}
                max={Number(plat.approval_threshold_usd)}
                step="any"
                inputMode="decimal"
                hint={`Payouts at or above this wait for an admin. Platform: ${usd(plat.approval_threshold_usd)}; empty uses it.`}
              />
            </Rule>
            <Rule
              title="Candidate data"
              effective={`${data.effective.candidate_retention_days} days`}
              own={data.company.candidate_retention_days != null}
            >
              <UnitInput
                label="Delete candidate data after"
                suffix="days"
                value={r}
                onChange={setRetention}
                placeholder={String(plat.candidate_retention_days)}
                min={1}
                max={plat.candidate_retention_days}
                inputMode="numeric"
                hint={`For people not hired. Platform: ${plat.candidate_retention_days} days; empty uses it.`}
              />
            </Rule>
          </div>
          {looser && <p className="rounded-xl bg-amber/10 px-3 py-2 text-sm text-amber ring-1 ring-amber/30">That's looser than the platform's rule, so it won't be accepted.</p>}
          <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
            <Button type="submit" disabled={busy}>{busy ? "Saving…" : "Save rules"}</Button>
            {data.updated_at && <p className="font-mono text-xs text-dim">Changed {when(data.updated_at)} by {data.updated_by}</p>}
          </div>
        </form>
      )}
    </Panel>
  );
}

function Rule({ title, effective, own, children }) {
  return (
    <div className="rounded-2xl bg-white/3 p-4 ring-1 ring-white/10 sm:p-5">
      <div className="flex items-center justify-between gap-3">
        <p className="text-sm font-medium">{title}</p>
        {own ? <Badge tone="lime">Company rule</Badge> : <Badge>Platform default</Badge>}
      </div>
      <p className="mt-3 text-xs text-mute">In effect</p>
      <p className="num mt-0.5 font-display text-3xl font-semibold tracking-tight">{effective}</p>
      <div className="mt-4">{children}</div>
    </div>
  );
}

// The kill switch lives on its own page now; this only points there.
function KillPointer() {
  const { navigate, status } = useApp();
  const kill = status?.kill?.data;
  const on = !!kill?.on;
  return (
    <button
      type="button"
      onClick={() => navigate("kill")}
      className={`reveal group flex w-full items-center gap-3 rounded-2xl px-4 py-3 text-left text-sm ring-1 transition ${on ? "bg-ember/10 ring-ember/30 hover:bg-ember/15" : "bg-white/3 ring-white/10 hover:bg-white/5"}`}
    >
      <span className={`pulse-dot size-2 shrink-0 rounded-full ${on ? "bg-ember text-ember" : "bg-lime text-lime"}`} aria-hidden="true" />
      <span className="min-w-0 flex-1 text-mute">
        The kill switch has its own page{kill ? (on ? <>: <span className="text-ember">it's on, everything is halted</span>.</> : ": it's off.") : "."}
      </span>
      <span className="flex shrink-0 items-center gap-1.5 text-fog">
        Kill switch <Icon name="arrow" size={14} className="transition group-hover:translate-x-0.5" />
      </span>
    </button>
  );
}
