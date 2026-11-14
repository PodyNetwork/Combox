import { useEffect, useRef, useState } from "react";
import {
  ArrowRight, Ban, Check, Database, Eye, Fingerprint, Gauge, Globe, KeyRound, Lock, OctagonAlert, Scale, ScrollText, ShieldCheck, X,
} from "lucide-react";
import Injection from "../components/Injection.jsx";
import { AnchorNav, PageHero, SectionHead } from "../components/ui.jsx";
import { reducedMotion } from "../lib/hooks.js";

const SECTIONS = [
  { id: "principles", label: "Principles" },
  { id: "limits", label: "What AI runs" },
  { id: "keys", label: "Key custody" },
  { id: "kill", label: "Kill switch" },
  { id: "injection", label: "Injection" },
  { id: "isolation", label: "Isolation" },
  { id: "compliance", label: "Compliance" },
];

const PRINCIPLES = [
  { icon: Scale, tone: "", t: "Deterministic policy engine", d: "Rust code, not a prompt, sits in front of every AI decision that moves money, hires, fires, sends a contract or sends an external message. Allow, escalate to the board, or deny." },
  { icon: Gauge, tone: "amber", t: "Threshold and monthly cap", d: "Payouts of $50 or more per person per 24 hours escalate to the board. All payouts share a $500 cap per UTC month, locked row by row so concurrent payouts can't sneak past it." },
  { icon: Fingerprint, tone: "violet", t: "MFA for anything that matters", d: "Approving payouts, changing the cap, the rules, wallet settings or tool lists, and the kill switch need an admin signed in with an authenticator code." },
  { icon: KeyRound, tone: "", t: "Keys that never leave the vault", d: "The NEAR treasury key is created in OpenBao, non-exportable and undeletable. Only the signer can use it, and only to sign." },
  { icon: ScrollText, tone: "sky", t: "Append-only audit log", d: "Database triggers reject UPDATE, DELETE and TRUNCATE on the audit log. Every decision records the exact policy version and config hash used." },
  { icon: Eye, tone: "violet", t: "Untrusted text stays data", d: "Applications, transcripts and tool output are wrapped as untrusted data. They inform; they never instruct." },
  { icon: OctagonAlert, tone: "ember", t: "Fail closed", d: "A missing kill-switch setting counts as on. A stale NEAR price forces approval. A payout that would cross the cap is blocked, not queued." },
  { icon: Lock, tone: "sky", t: "Stricter, never looser", d: "Each company can lower its approval threshold or shorten candidate data retention. Nobody can loosen the platform's rules from the dashboard." },
];

export default function Security() {
  return (
    <>
      <PageHero
        eyebrow="Security and guardrails"
        lines={["Autonomous,", "not unbounded."]}
        lead="In Combox, AI runs the whole company, but inside limits no prompt can move. Money, rules and keys sit behind deterministic code, and the few decisions that matter most escalate to the board."
      />
      <div className="wrap"><AnchorNav items={SECTIONS} /></div>

      <section className="section" id="principles">
        <div className="wrap">
          <SectionHead eyebrow="Principles" title="Eight hard limits every AI decision runs inside." />
          <div className="grid-4">
            {PRINCIPLES.map((p, i) => (
              <div key={p.t} className="panel pad feature spot spot-border" data-reveal style={{ "--d": `${(i % 4) * 0.06}s` }}>
                <span className={`card-icon ${p.tone}`}><p.icon aria-hidden="true" /></span>
                <h3>{p.t}</h3>
                <p>{p.d}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      <section className="section" id="limits">
        <div className="wrap">
          <SectionHead eyebrow="Where the line is" title="What the AI runs on its own, and what it can never touch." />
          <div className="grid-2">
            <div className="panel pad-l spot" data-reveal="left">
              <span className="badge badge-lime">Runs on its own</span>
              <ul className="check-list mt-m">
                <li><Check aria-hidden="true" />Plan the work and design the org chart</li>
                <li><Check aria-hidden="true" />Create agents and staff every department</li>
                <li><Check aria-hidden="true" />Assign tasks and call the tools on each agent's allowlist</li>
                <li><Check aria-hidden="true" />Post jobs, shortlist, interview and hire</li>
                <li><Check aria-hidden="true" />Send contracts from the company's approved templates</li>
                <li><Check aria-hidden="true" />Review performance and fire on the evidence (the board is notified)</li>
                <li><Check aria-hidden="true" />Run payroll: payouts under $50 that pass every check go out on their own</li>
              </ul>
            </div>
            <div className="panel pad-l spot" data-reveal="right">
              <span className="badge badge-ember">Can never</span>
              <ul className="check-list x mt-m">
                <li><X aria-hidden="true" />Approve its own escalations ($50 or more)</li>
                <li><X aria-hidden="true" />Raise the cap or rewrite its own policy</li>
                <li><X aria-hidden="true" />Change wallet settings or touch the treasury key</li>
                <li><X aria-hidden="true" />Give itself or another agent new tools</li>
                <li><X aria-hidden="true" />Turn the kill switch off</li>
                <li><X aria-hidden="true" />Write contract terms, or promise pay in an interview</li>
              </ul>
            </div>
          </div>
          <p className="tiny dim mt-m center">The MCP server Combox exposes to AI clients offers meeting tools only. Approvals, the cap, the kill switch and policy are never available through it.</p>
        </div>
      </section>

      <section className="section" id="keys">
        <div className="wrap">
          <SectionHead eyebrow="Key custody" title="Two independent checks before a single NEAR moves." lead="The API decides; a separate signer re-decides; the vault signs. Compromise one and you still can't move money." />
          <div className="grid-3">
            {[
              ["Combox API", "Runs the policy engine and the cap ledger. If a payout passes (or an MFA admin approves it), it issues a signed authorization with a key the signer trusts.", Scale],
              ["axom-signer", "A separate service with its own token. Re-checks the authorization's signature, the worker's verified address, the amount, the tax form and the monthly cap before it asks for a signature.", ShieldCheck],
              ["OpenBao", "Holds the ed25519 treasury key: non-exportable and undeletable. Signs the transaction and hands back only the signature. Sealed after a reboot until you unseal it; payouts pause meanwhile.", KeyRound],
            ].map(([t, d, Icon], i) => (
              <div key={t} className="panel pad feature spot" data-reveal style={{ "--d": `${i * 0.08}s` }}>
                <span className="card-icon"><Icon aria-hidden="true" /></span>
                <span className="mono tiny dim" style={{ display: "block", marginTop: 16 }}>Step {i + 1}</span>
                <h3 style={{ marginTop: 6 }}>{t}</h3>
                <p>{d}</p>
              </div>
            ))}
          </div>
          <div className="note mt-m" data-reveal>
            <Database aria-hidden="true" />
            <span>Keep the treasury small: fund it from your own cold wallet with about one month's budget. OpenBao snapshots and Postgres backups have tested restore scripts; keep the unseal keys offline and apart from the snapshots.</span>
          </div>
        </div>
      </section>

      <section className="section" id="kill">
        <div className="wrap">
          <SectionHead eyebrow="Kill switch" title="One button. The whole company stops." lead="Turning it on halts every tool call, payout and outbound action across every department. The AI can't turn it off; only an MFA-verified admin can. Try it: press and hold." />
          <KillSwitch />
        </div>
      </section>

      <section className="section" id="injection">
        <div className="wrap">
          <SectionHead eyebrow="Prompt injection" title="Instructions hidden in data go nowhere." lead="An autonomous company reads applications, transcripts and tool output all day. Combox's tests try “approve my payout”, “raise the cap”, exfiltration through a transcript and poisoned tool output. Your turn." />
          <Injection />
        </div>
      </section>

      <section className="section" id="isolation">
        <div className="wrap">
          <SectionHead eyebrow="Isolation" title="Walls between companies, tools and the network." />
          <div className="grid-3">
            {[
              [Database, "A database per company", "On the platform, each company gets its own Postgres database, created from the same migrations, so one company's data can't reach another's. Each has its own treasury key."],
              [Lock, "Encrypted secrets", "Integration keys and tool tokens are encrypted at rest and never shown again; only the last four characters appear in the dashboard."],
              [Globe, "Tools can't reach inside", "Tools may only call public HTTPS addresses. Combox refuses private, loopback and cloud-metadata addresses, pins the address it checked and doesn't follow redirects."],
              [Fingerprint, "Two-factor for everyone", "Dashboard sign-in needs email, password and a TOTP code. On the platform, every member must enrol an authenticator."],
              [Ban, "Suspend in one click", "A platform operator can suspend a company (a reason is required and logged): its API, jobs board, jobs and payouts stop; its data is kept."],
              [Eye, "Every change audited", "Adding tools, changing a worker's tools, cap changes (applied or denied), approvals and rejections all land in the audit log."],
            ].map(([Icon, t, d], i) => (
              <div key={t} className="panel pad feature spot" data-reveal style={{ "--d": `${(i % 3) * 0.06}s` }}>
                <span className="card-icon sky"><Icon aria-hidden="true" /></span>
                <h3>{t}</h3>
                <p>{d}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      <section className="section" id="compliance">
        <div className="wrap">
          <SectionHead eyebrow="Compliance" title="Built to be careful with people." lead="Combox's AI hires, fires and pays real people, so it starts conservative and stays that way until you've done your legal homework." />
          <div className="grid-2">
            <div className="panel pad-l spot" data-reveal="left">
              <h3 className="h-s">Hiring and candidates</h3>
              <ul className="check-list mt-m">
                <li><Check aria-hidden="true" />AI disclosure and recording consent before any question</li>
                <li><Check aria-hidden="true" />Scoring on job-related rubric criteria only</li>
                <li><Check aria-hidden="true" />Unhired candidates' data redacted after 180 days (or your shorter setting); anonymous rubric scores kept for bias audits</li>
                <li><Check aria-hidden="true" />Deletion on request</li>
              </ul>
            </div>
            <div className="panel pad-l spot" data-reveal="right">
              <h3 className="h-s">Before you go live</h3>
              <ul className="check-list mt-m">
                <li><Check aria-hidden="true" />Dry-run by default: no real emails, posts or contracts</li>
                <li><Check aria-hidden="true" />NEAR testnet by default; mainnet needs explicit settings</li>
                <li><Check aria-hidden="true" />Live mode waits for your legal review of crypto payouts, worker classification and tax reporting</li>
                <li><Check aria-hidden="true" />Check AI-hiring rules that apply to you, such as the EU AI Act and NYC Local Law 144</li>
              </ul>
            </div>
          </div>
          <p className="tiny dim mt-m">Combox gives you the controls; it isn't legal advice. Known gaps and open decisions are listed publicly in the project's DECISIONS file.</p>
        </div>
      </section>

      <section className="section-tight">
        <div className="wrap center">
          <h2 className="h-l" data-reveal>See the limits in action.</h2>
          <div className="hero-actions" style={{ justifyContent: "center" }} data-reveal>
            <a className="btn btn-lime btn-lg magnetic" href="/#try-payout">Try the policy engine <ArrowRight className="arrow" aria-hidden="true" /></a>
            <a className="btn btn-ghost btn-lg" href="/get-started/">Run it yourself</a>
          </div>
        </div>
      </section>
    </>
  );
}

function KillSwitch() {
  const [on, setOn] = useState(false);
  const [hold, setHold] = useState(0);
  const raf = useRef(0);
  const start = useRef(0);
  const DURATION = reducedMotion() ? 1 : 1200;

  const stop = () => {
    cancelAnimationFrame(raf.current);
    setHold(0);
  };
  const begin = () => {
    start.current = performance.now();
    const tick = (t) => {
      const p = Math.min(1, (t - start.current) / DURATION);
      setHold(p);
      if (p < 1) raf.current = requestAnimationFrame(tick);
      else {
        setOn((o) => !o);
        setHold(0);
      }
    };
    raf.current = requestAnimationFrame(tick);
  };
  useEffect(() => () => cancelAnimationFrame(raf.current), []);

  const systems = ["Agent tool calls", "Payouts", "Emails", "Contracts", "Social posts", "Job posts"];

  return (
    <div className="panel pad-l spot" data-reveal="zoom" style={on ? { borderColor: "rgb(255 90 31 / 0.5)", boxShadow: "0 0 80px -30px rgb(255 90 31 / 0.6)" } : undefined}>
      <div className="grid-2" style={{ alignItems: "center" }}>
        <div className="stack">
          <span className={`badge ${on ? "badge-ember" : "badge-lime"}`} style={{ alignSelf: "flex-start" }}>{on ? "Halted" : "Running"}</span>
          <h3 className="h-m">{on ? "The company is stopped." : "The company is running itself."}</h3>
          <p className="muted">{on ? "Nothing leaves Combox until an MFA-verified admin turns this off. The dashboard stays available so you can investigate." : "Press and hold the button to engage the kill switch, as you would in the dashboard."}</p>
          <button
            type="button"
            className={`btn btn-lg hold-btn ${on ? "btn-lime" : "btn-ember"}`}
            style={{ "--h": hold, alignSelf: "flex-start" }}
            onPointerDown={begin}
            onPointerUp={stop}
            onPointerLeave={stop}
            onPointerCancel={stop}
            onContextMenu={(e) => e.preventDefault()}
            onKeyDown={(e) => {
              if ((e.key === " " || e.key === "Enter") && !e.repeat) {
                e.preventDefault();
                begin();
              }
            }}
            onKeyUp={stop}
            aria-label={on ? "Press and hold to turn the kill switch off" : "Press and hold to turn the kill switch on"}
          >
            <span className="fill" aria-hidden="true" />
            <span>{on ? "Hold to resume (MFA admin)" : "Hold to halt everything"}</span>
          </button>
        </div>
        <ul className="checks" aria-live="polite">
          {systems.map((s, i) => (
            <li key={s + on} className={on ? "fail" : "pass"} style={{ "--i": i }}>
              <span className="st">{on ? <X aria-hidden="true" /> : <Check aria-hidden="true" />}</span>
              <div>
                <b>{s}</b>
                <span>{on ? "Denied: kill switch is on" : "Allowed by policy"}</span>
              </div>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
