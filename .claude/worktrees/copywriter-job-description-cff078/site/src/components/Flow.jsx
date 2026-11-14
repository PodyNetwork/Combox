import { Fragment, useState } from "react";
import {
  BadgeCheck, Banknote, Bot, Briefcase, ChevronRight, ClipboardList, FileSignature, Gauge, KeyRound, Landmark, LayoutDashboard,
  ListChecks, Network, Scale, ScrollText, ShieldCheck, Target, UserMinus, Video,
} from "lucide-react";

// The architecture from the README, one lane per loop. Each node explains
// what that part does and where it lives in the code.
const LANES = [
  {
    id: "work",
    label: "Work",
    nodes: [
      { id: "plan", icon: Target, name: "Mission", info: "Your goal, budget, deadline and objectives (config/master_plan.json, or the onboarding form). The only thing a human writes; the AI runs everything after it." },
      { id: "planner", icon: Network, name: "AI planner", info: "Maps each objective to a department template, builds the org chart, creates one first task per objective and opens hiring requests for human-only work (axom-planner)." },
      { id: "chart", icon: ClipboardList, name: "Org chart + tasks", info: "Departments, teams, roles with skills, tools, pay rates and KPIs. Task state machine: queued → assigned → in progress → done, with every transition audited." },
      { id: "routing", icon: ListChecks, name: "Operations", info: "The company assigns each task itself, to the worker with the right tools and skills and the lightest load. No match → a hiring request." },
      { id: "workers", icon: Bot, name: "AI workers / freelancers", info: "AI agents think with your company's Claude model and act only through tools on their allowlist (MCP servers and webhooks). Freelancers work through the brief portal." },
    ],
  },
  {
    id: "hiring",
    label: "Hiring",
    nodes: [
      { id: "req", icon: Briefcase, name: "Hiring request", info: "Opened by the AI when no current worker can take a task, for example logo design." },
      { id: "jobs", icon: ScrollText, name: "ForeMan Jobs", info: "The company's own public jobs board at /jobs, posted to by the AI. Candidates apply with a PDF CV, no account needed. Shortlisted by skill match. (Or Upwork, paid through Upwork.)" },
      { id: "interview", icon: Video, name: "AI interview", info: "Video interview over LiveKit, bundled with ForeMan. Disclosure and recording consent first, job questions, no talk of pay. Transcript, summary and rubric scores are stored." },
      { id: "contract", icon: FileSignature, name: "DocuSeal contract", info: "The AI makes the hire, always with one of your pre-approved templates. It never writes contract terms. The board is notified when a contract goes out." },
    ],
  },
  {
    id: "perf",
    label: "Performance",
    nodes: [
      { id: "records", icon: ClipboardList, name: "Task records", info: "Completion, deadlines, cost per task and whether the work passed its checks. Results, not self-reports." },
      { id: "reviews", icon: Scale, name: "Reviews", info: "Every 10 finished tasks or every 7 days. Run by the AI. Below-threshold reviews put a worker on probation. 10% go to the board's spot-check queue." },
      { id: "firing", icon: UserMinus, name: "Firing", info: "Only after 10+ completed tasks, 14+ days of data and two bad periods in a row. The AI decides; open tasks are reassigned; the board is notified with the reason, metrics and an audit link." },
    ],
  },
  {
    id: "money",
    label: "Money",
    nodes: [
      { id: "payroll", icon: Banknote, name: "Payroll", info: "Run by the company every week: accepted work × rate for each worker. Workers need a proven NEAR address and a tax form on file before their first payout." },
      { id: "policy", icon: Gauge, name: "Policy engine + cap", info: "Deterministic Rust, not a model. Kill switch, price freshness, the $50 / 24h approval threshold and the $500 monthly cap. Every decision records the policy version it used." },
      { id: "attest", icon: BadgeCheck, name: "Signed authorization", info: "Payouts that pass get a signed attestation; escalated payouts get one only after an MFA admin on the board approves." },
      { id: "dash", icon: LayoutDashboard, name: "Board escalation", info: "The one decision the AI hands up: payouts of $50 or more wait here. Sign-in with email, password and an authenticator code (TOTP). Approvals expire after 7 days." },
      { id: "signer", icon: ShieldCheck, name: "axom-signer", info: "A separate service. Re-checks the authorization, the verified address, the amount, the tax form and the cap before asking the vault to sign." },
      { id: "bao", icon: KeyRound, name: "OpenBao", info: "Holds the treasury's ed25519 key: non-exportable, undeletable. It signs; it never hands the key out." },
      { id: "near", icon: Landmark, name: "NEAR", info: "The transfer lands on chain. Testnet by default; mainnet needs live mode, an explicit RPC URL and your legal review." },
    ],
  },
];

export default function Flow() {
  const [lane, setLane] = useState("money");
  const L = LANES.find((l) => l.id === lane);
  const [active, setActive] = useState(L.nodes[0].id);
  const node = L.nodes.find((n) => n.id === active) || L.nodes[0];
  const Icon = node.icon;

  return (
    <div className="panel pad-l spot" data-reveal>
      <div className="row" style={{ justifyContent: "space-between" }}>
        <div className="tabs" role="tablist" aria-label="Loops">
          {LANES.map((l) => (
            <button
              key={l.id}
              role="tab"
              type="button"
              className="tab"
              aria-selected={lane === l.id}
              onClick={() => {
                setLane(l.id);
                setActive(l.nodes[0].id);
              }}
            >
              {l.label}
            </button>
          ))}
        </div>
        <span className="tiny dim">Click any step</span>
      </div>
      <div className="flow mt-m dim-others" key={lane}>
        <div className="flow-row" role="list">
          {L.nodes.map((n, i) => {
            const NIcon = n.icon;
            return (
              <Fragment key={n.id}>
                <button
                  type="button"
                  role="listitem"
                  className={`flow-node ${active === n.id ? "on" : ""}`}
                  onClick={() => setActive(n.id)}
                  onMouseEnter={() => setActive(n.id)}
                  onFocus={() => setActive(n.id)}
                  aria-pressed={active === n.id}
                  style={{ animation: `nodeIn 0.5s var(--ease) ${i * 60}ms both` }}
                >
                  <NIcon aria-hidden="true" />
                  {n.name}
                </button>
                {i < L.nodes.length - 1 && <span className="flow-arrow" aria-hidden="true"><ChevronRight /></span>}
              </Fragment>
            );
          })}
        </div>
        <div className="flowline" aria-hidden="true" />
        <div className="flow-info" aria-live="polite" key={node.id} style={{ animation: "feedIn 0.4s var(--ease)" }}>
          <div className="row">
            <span className="card-icon"><Icon aria-hidden="true" /></span>
            <h3 className="h-s">{node.name}</h3>
          </div>
          <p className="muted mt-s">{node.info}</p>
        </div>
      </div>
    </div>
  );
}
