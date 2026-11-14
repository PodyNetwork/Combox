import { useEffect, useState } from "react";
import { ArrowRight, ArrowUpRight, Check, Cloud, Laptop, Server, Sparkles } from "lucide-react";
import Terminal from "../components/Terminal.jsx";
import { CodeBlock, PageHero, SectionHead } from "../components/ui.jsx";
import { DASHBOARD_HOME, REPO_URL, TRIAL_DAYS } from "../lib/config.js";

const PATHS = [
  { id: "demo", label: "Try the demo", icon: Laptop },
  { id: "self-host", label: "Self-host", icon: Server },
  { id: "platform", label: "Sign up", icon: Sparkles },
  { id: "coolify", label: "Deploy on a VPS", icon: Cloud },
];

const ENV_STEPS = [
  { t: "Minimum: the company's core, jobs board, dashboard, hiring loop", d: "DATABASE_URL (Postgres; tables are created on first start) and AXOM_OWNER_EMAIL. AXOM_LIVE=false keeps every adapter in dry-run.", code: "cp .env.example .env" },
  { t: "Board sign-in and escalations", d: "Supabase URL and JWKS (or JWT secret), the dashboard's Supabase anon key, TOTP MFA enabled, yourself in axom.admins, and two key pairs from axom keygen for attestations and approvals.", code: "cd axom && cargo run -p axom-bin -- keygen" },
  { t: "Payouts on NEAR testnet", d: "Run and initialise OpenBao, then the treasury setup script. It prints the treasury address (fund it with testnet NEAR) and the signer token, which goes in OPENBAO_TOKEN (never a file). Add a CoinGecko key for the NEAR price.", code: "infra/openbao/setup-treasury.sh" },
  { t: "AI interviews", d: "LiveKit and the AI interviewer ship with ForeMan and start with Docker. Add the speech and Claude keys from the dashboard (Integrations → AI models) and public URLs candidates can reach." },
  { t: "Email, contracts, tax forms, CRM", d: "SMTP, DocuSeal with your approved template ids, and the CRM. Connect MCP servers for the AI workforce on the dashboard's Tools page. Still dry-run." },
  { t: "Live", d: "AXOM_LIVE=true lets the company act in the real world. Only after your legal review. Mainnet additionally needs NEAR_NETWORK=mainnet and an explicit NEAR_RPC_URL." },
];

export default function GetStarted() {
  // The prerendered page shows the demo tab; a #self-host or #platform link switches after hydration.
  const [tab, setTab] = useState("demo");
  useEffect(() => {
    const on = () => {
      const h = location.hash.slice(1);
      if (PATHS.some((p) => p.id === h)) setTab(h);
    };
    on();
    addEventListener("hashchange", on);
    return () => removeEventListener("hashchange", on);
  }, []);
  const choose = (id) => {
    setTab(id);
    history.replaceState(null, "", `#${id}`);
  };

  return (
    <>
      <PageHero eyebrow="Get started" lines={["A self-running company", "in one command."]} lead="Watch a fully autonomous company run itself on your laptop in about ten minutes, launch your own on your servers, or sign up for the hosted platform." />
      <section className="section-tight" style={{ paddingTop: 0 }}>
        <div className="wrap">
          <div className="tabs" role="tablist" aria-label="Ways to start">
            {PATHS.map((p) => (
              <button key={p.id} id={`tab-${p.id}`} role="tab" type="button" className="tab" aria-selected={tab === p.id} aria-controls={`panel-${p.id}`} onClick={() => choose(p.id)}>
                <p.icon aria-hidden="true" />{p.label}
              </button>
            ))}
          </div>
          <div className="mt-l" role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`} key={tab}>
            <div className="tabpanel">
              {tab === "demo" && <Demo />}
              {tab === "self-host" && <SelfHost />}
              {tab === "platform" && <Platform />}
              {tab === "coolify" && <Coolify />}
            </div>
          </div>
        </div>
      </section>
      <section className="section">
        <div className="wrap">
          <SectionHead eyebrow="For AI clients" title="Plug the company's meetings into your own assistant." lead="ForeMan serves an MCP endpoint with meeting tools only: create_meeting, dispatch_agent, get_transcript, get_summary, send_summary. Approvals, the cap, the kill switch and policy are never exposed." />
          <div data-reveal style={{ maxWidth: 860 }}>
            <CodeBlock code={'# Claude Code, for example\nclaude mcp add --transport http axom https://api.example.com/mcp \\\n  --header "Authorization: Bearer $AXOM_MCP_TOKEN"'} />
          </div>
        </div>
      </section>
    </>
  );
}

function Demo() {
  return (
    <div className="grid-2" style={{ alignItems: "start" }}>
      <div className="stack" style={{ gap: 24 }}>
        <div>
          <h2 className="h-m">Judge mode: a company running itself, locally.</h2>
          <p className="muted mt-s">Builds and starts everything, lets the AI run the company through the demo scenario and opens the ForeMan website in your browser; its Sign in takes you straight into the dashboard. Outside services are mocked; payouts are real, on NEAR testnet.</p>
        </div>
        <div>
          <div className="small muted">You'll need</div>
          <div className="row mt-s" style={{ gap: 8 }}>
            {["Rust", "Node.js 20+", "Python 3", "Docker", "curl"].map((r) => <span key={r} className="chip">{r}</span>)}
          </div>
        </div>
        <div>
          <div className="small muted">Then, from the repository folder</div>
          <div className="mt-s"><CodeBlock code="./scripts/start demo" /></div>
          <p className="tiny dim mt-s">The first run takes a few minutes to build. <span className="kbd">Ctrl</span> + <span className="kbd">C</span> stops everything; <span className="mono">./scripts/stop</span> cleans up anything left running.</p>
        </div>
        <div>
          <div className="small muted">What you'll see</div>
          <ol className="timeline mt-s">
            {[
              "The AI builds an org chart from “Launch a SaaS product in 90 days” and assigns the work",
              "It posts a job after a policy check",
              "It runs an AI interview: disclosure, consent, questions, and a refusal to talk pay",
              "It fires a seeded low performer after two bad periods and notifies the board",
              "It pays $5 on NEAR testnet, escalates a $60 payout to the board, and is blocked by the cap on $400",
              "A final report in judge/out/report.json",
            ].map((t, i) => (
              <li key={t}><span className="n">{i + 1}</span><div><p style={{ marginTop: 10 }}>{t}</p></div></li>
            ))}
          </ol>
        </div>
        <div>
          <div className="small muted">Act as the board: approve the escalated payout from the dashboard, or from another terminal</div>
          <div className="mt-s"><CodeBlock code="./judge/approve.sh" /></div>
        </div>
      </div>
      <div style={{ position: "sticky", top: "calc(var(--nav-h) + 24px)" }}>
        <Terminal />
      </div>
    </div>
  );
}

function SelfHost() {
  const [done, setDone] = useState([]);
  // Saved progress is read after hydration (the prerendered page starts empty).
  useEffect(() => {
    try {
      setDone(JSON.parse(localStorage.getItem("axom-site-setup") || "[]"));
    } catch {
      /* storage blocked or corrupt: start empty */
    }
  }, []);
  const toggle = (i) =>
    setDone((d) => {
      const next = d.includes(i) ? d.filter((x) => x !== i) : [...d, i];
      try {
        localStorage.setItem("axom-site-setup", JSON.stringify(next));
      } catch {
        /* storage blocked: progress just isn't remembered */
      }
      return next;
    });
  const pct = Math.round((done.length / ENV_STEPS.length) * 100);

  return (
    <div className="grid-2" style={{ alignItems: "start" }}>
      <div className="stack" style={{ gap: 20 }}>
        <h2 className="h-m">Run your own autonomous company.</h2>
        <p className="muted">Copy the example settings, then fill them in step by step. Each step unlocks more of the system; parts whose settings are missing are skipped with a note.</p>
        <CodeBlock code={"cp .env.example .env\ndocker compose up --build"} />
        <p className="small muted">Add the signer once OpenBao is set up:</p>
        <CodeBlock code="docker compose --profile payouts up --build" />
        <p className="tiny dim">That starts Postgres, the API and scheduler, the dashboard (:5174), the jobs board (:5173/jobs), LiveKit and the AI interviewer. Policy values live in config/policy.toml; your plan in config/master_plan.json.</p>
        <a className="arrow-link" href={`${REPO_URL}/blob/HEAD/docs/SETUP.md`} target="_blank" rel="noreferrer">Full setup guide <ArrowUpRight aria-hidden="true" /></a>
      </div>
      <div className="panel pad-l">
        <div className="row" style={{ justifyContent: "space-between" }}>
          <span className="demo-tag">Your progress</span>
          <span className="mono small">{pct}%</span>
        </div>
        <div className="meter"><i className="m-used" style={{ width: `${pct}%` }} /></div>
        <ul className="checks mt-m" style={{ gap: 8 }}>
          {ENV_STEPS.map((s, i) => (
            <li key={s.t} className={done.includes(i) ? "pass" : "todo"} style={{ "--i": i, opacity: 1, cursor: "pointer" }} onClick={() => toggle(i)}>
              <button type="button" className="st" style={{ border: 0, cursor: "pointer" }} aria-pressed={done.includes(i)} aria-label={`Mark step ${i + 1} ${done.includes(i) ? "not done" : "done"}`} onClick={(e) => { e.stopPropagation(); toggle(i); }}>
                {done.includes(i) ? <Check aria-hidden="true" /> : <span className="mono tiny">{i + 1}</span>}
              </button>
              <div>
                <b>{s.t}</b>
                <span>{s.d}</span>
                {s.code && <code className="mono tiny lime" style={{ display: "block", marginTop: 6 }}>{s.code}</code>}
              </div>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}

function Platform() {
  const steps = [
    ["Sign up", "Email and password, then set up an authenticator app. Two-factor is required for everyone, because the company you launch pays real people."],
    ["Create your company", `A name and an address for your links (like /t/acme/jobs). The ${TRIAL_DAYS}-day free trial starts now.`],
    ["Give it a mission", "Goal, budget, deadline, objectives. The AI builds the departments, creates the agents, sets the tasks and opens hiring requests. You can skip this and do it later."],
    ["Hand over the tools", "Your email (SMTP), DocuSeal, Postiz, opendesk, Warmbly, Upwork, your Claude key and MCP tools. Keys are encrypted; only the last four characters are shown."],
    ["Create your NEAR treasury", "In Settings. Its key is created in OpenBao, can't be exported and only the signer can use it. Fund it from your wallet."],
    ["Invite your board", "By email from Members. Invitees join when they sign up with that address. The AI handles the rest."],
    ["Pay in NEAR", "Billing quotes the plan in NEAR. Send it from any wallet, paste the hash, done."],
  ];
  return (
    <div className="grid-2" style={{ alignItems: "start" }}>
      <div className="stack" style={{ gap: 20 }}>
        <h2 className="h-m">Your autonomous company on hosted ForeMan.</h2>
        <p className="muted">The AI runs every department exactly as it does self-hosted (strategy, hiring, interviews, payroll, reviews), inside the same limits (the $50 escalation threshold, your own monthly cap, the kill switch), separately for each company.</p>
        <div className="row">
          <a className="btn btn-lime btn-lg magnetic" href={DASHBOARD_HOME}>Launch your company <ArrowRight className="arrow" aria-hidden="true" /></a>
          <a className="btn btn-ghost btn-lg" href="/pricing/">Pricing</a>
        </div>
      </div>
      <ol className="timeline">
        {steps.map(([t, d], i) => (
          <li key={t}><span className="n">{i + 1}</span><div><h4>{t}</h4><p>{d}</p></div></li>
        ))}
      </ol>
    </div>
  );
}

function Coolify() {
  const steps = [
    ["Install Coolify", "On an Ubuntu 22.04+ VPS with Docker. Point DNS at it and turn on automatic TLS, security updates and encrypted S3 backups for every database."],
    ["Supabase", "Add Coolify's Supabase service, enable TOTP MFA in Auth, create your admin user and add yourself to axom.admins."],
    ["OpenBao", "Deploy it with TLS, initialise once, keep the unseal keys and root token offline, unseal, then run the treasury setup script and fund the printed address with at most a month's budget."],
    ["LiveKit", "Part of the ForeMan service definition, with host networking. Open TCP 7880–7881, UDP 50000–60000 and TURN on UDP 3478 / TLS 5349."],
    ["DocuSeal and opendesk", "Deploy DocuSeal and set up opendesk, hosted or your own (and Postiz if your agents will post). Create your approved templates: freelancer contract, termination notice, W-9, W-8BEN."],
    ["ForeMan", "Generate the authorization keys, paste every variable into Coolify's environment editor, add the signer token and deploy the API, signer, meeting agent, dashboard and participant site."],
  ];
  return (
    <div className="grid-2" style={{ alignItems: "start" }}>
      <div className="stack" style={{ gap: 20 }}>
        <h2 className="h-m">Production on your own VPS.</h2>
        <p className="muted">Everything is self-hosted on Coolify except Warmbly and the Upwork MCP, which their providers host. Backups: nightly Postgres dumps and OpenBao snapshots, with restore scripts that prove both work.</p>
        <CodeBlock code={"BAO_ADDR=https://openbao.internal:8200 BAO_TOKEN=<owner token> \\\n  infra/openbao/setup-treasury.sh"} />
        <a className="arrow-link" href={`${REPO_URL}/tree/HEAD/infra/coolify`} target="_blank" rel="noreferrer">Coolify service definitions <ArrowUpRight aria-hidden="true" /></a>
      </div>
      <ol className="timeline">
        {steps.map(([t, d], i) => (
          <li key={t}><span className="n">{i + 1}</span><div><h4>{t}</h4><p>{d}</p></div></li>
        ))}
      </ol>
    </div>
  );
}
