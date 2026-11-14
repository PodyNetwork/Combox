import { useEffect, useRef, useState } from "react";
import { ArrowRight, BadgeCheck, Banknote, Briefcase, Check, FileSignature, Gauge, Hourglass, KeyRound, Landmark, ScrollText, Video } from "lucide-react";

const STEPS = [
  {
    k: "Mission",
    title: "You set the mission. Once.",
    body: "A goal, a budget, a deadline and a handful of objectives. That's the last instruction anyone has to give. From here, AI runs the company.",
    points: ["“Launch a SaaS product in 90 days, $450 budget”", "Change it any time from Settings", "Fund the company's NEAR treasury with about a month's budget"],
    Visual: PlanVisual,
  },
  {
    k: "Organise",
    title: "AI designs the organisation.",
    body: "The AI turns each objective into a department with teams and roles: agents for work they can do, human roles for work they can't.",
    points: ["Departments, teams, roles, KPIs and pay rates", "The AI creates its own agents on the spot", "Human roles open hiring requests"],
    Visual: OrgVisual,
  },
  {
    k: "Operate",
    title: "AI runs the day-to-day.",
    body: "Engineering ships, marketing posts, sales reaches out. The company assigns every task to the worker with the right tools and skills and the lightest load. Agents act only through tools on their own allowlist.",
    points: ["Tools are MCP servers and webhooks you connect", "Each tool call is checked against the worker's list", "Tool output is stored as untrusted data"],
    Visual: RouteVisual,
  },
  {
    k: "Hire",
    title: "AI recruits the humans it needs.",
    body: "When no agent can do a task, the AI posts the job, interviews applicants by video and sends a contract from your approved templates.",
    points: ["ForeMan Jobs: the company's public board, no account needed to apply", "Recording consent before any question", "Contracts only from pre-approved templates"],
    Visual: HireVisual,
  },
  {
    k: "Review",
    title: "AI manages performance.",
    body: "The AI judges every worker on results: tasks done, deadlines met, cost and checks passed. It lets persistent underperformers go and tells the board why.",
    points: ["Reviews every 10 tasks or 7 days", "Firing needs 10+ tasks, 14+ days and two bad periods in a row", "Open tasks are reassigned automatically"],
    Visual: ReviewVisual,
  },
  {
    k: "Pay",
    title: "AI runs payroll, inside hard limits.",
    body: "The company works out what each person earned and pays them in NEAR. Small payouts go out on their own; anything from $50 escalates to the board; nothing crosses the monthly cap.",
    points: ["Signed in OpenBao; the key never leaves the vault", "Proven address and tax form required", "Escalations signed off in the dashboard with MFA"],
    Visual: PayVisual,
  },
];

export default function Lifecycle() {
  const [active, setActive] = useState(0);
  const refs = useRef([]);

  useEffect(() => {
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) if (e.isIntersecting) setActive(Number(e.target.dataset.i));
      },
      { rootMargin: "-45% 0px -45% 0px" },
    );
    refs.current.forEach((el) => el && io.observe(el));
    return () => io.disconnect();
  }, []);

  return (
    <div className="life">
      <div className="life-visual" aria-hidden="true">
        {STEPS.map((s, i) => (
          <div key={s.k} className={`life-stage ${i === active ? "on" : ""}`}>
            <s.Visual on={i === active} key={i === active ? "on" : "off"} />
          </div>
        ))}
      </div>
      <ol className="life-steps">
        {STEPS.map((s, i) => (
          <li key={s.k} ref={(el) => (refs.current[i] = el)} data-i={i} className={`life-step ${i === active ? "on" : ""}`}>
            <span className="k"><b>{String(i + 1).padStart(2, "0")}</b>{s.k}</span>
            <h3>{s.title}</h3>
            <p>{s.body}</p>
            <ul>
              {s.points.map((p) => (
                <li key={p}><Check aria-hidden="true" />{p}</li>
              ))}
            </ul>
            <div className="life-mobile-visual" aria-hidden="true"><s.Visual on /></div>
          </li>
        ))}
      </ol>
    </div>
  );
}

function Frame({ title, badge, children }) {
  return (
    <div className="panel">
      <div className="vis-head"><span>{title}</span>{badge}</div>
      <div className="vis-body">{children}</div>
    </div>
  );
}

function PlanVisual() {
  return (
    <Frame title="master_plan.json" badge={<span className="badge badge-lime">Valid</span>}>
      <pre className="plan-doc" style={{ margin: 0, whiteSpace: "pre-wrap" }}>
{"{\n  "}<span className="k">"goal"</span>: <span className="s">"Launch a SaaS product in 90 days"</span>,{"\n  "}
<span className="k">"budget_usd"</span>: <span className="n">"450"</span>,{"\n  "}
<span className="k">"deadline"</span>: <span className="s">"2027-02-06"</span>,{"\n  "}
<span className="k">"objectives"</span>: [{"\n    "}
<span className="s">"Build the MVP web app"</span>,{"\n    "}
<span className="s">"Deploy production infrastructure"</span>,{"\n    "}
<span className="s">"Design the brand and logo"</span>,{"\n    "}
<span className="s">"Run the launch marketing campaign"</span>,{"\n    "}
<span className="s">"Start outbound sales"</span>,{"\n    "}
<span className="s">"Set up contracts and compliance"</span>{"\n  ]\n}"}
      </pre>
    </Frame>
  );
}

function OrgVisual() {
  return (
    <Frame title="Org chart · designed by AI" badge={<span className="badge badge-violet">5 AI · 1 human</span>}>
      <div className="mini-tree">
        <div className="mini-node" style={{ background: "var(--lime)", color: "var(--ink)", fontWeight: 600 }}>Launch a SaaS product</div>
        <div className="mini-row">
          {["Engineering", "Marketing", "Sales", "Design", "Operations"].map((d) => (
            <div className="mini-node" key={d}>{d}</div>
          ))}
        </div>
        <div className="mini-row">
          <div className="mini-node ai">Engineer Agent ×2</div>
          <div className="mini-node ai">Content Marketer</div>
          <div className="mini-node ai">Sales Dev Agent</div>
          <div className="mini-node human">Brand Designer · hiring</div>
          <div className="mini-node ai">Operations Agent</div>
        </div>
      </div>
    </Frame>
  );
}

function RouteVisual() {
  const rows = [
    ["Build the MVP web app", "Engineer Agent 1", "code_repo"],
    ["Deploy production infra", "Engineer Agent 2", "code_repo"],
    ["Launch campaign", "Content Marketer", "postiz"],
    ["Start outbound sales", "Sales Dev Agent", "warmbly · crm"],
    ["Design the logo", "Hiring request", "—"],
  ];
  return (
    <Frame title="Operations" badge={<span className="badge badge-lime">Assigned by AI</span>}>
      {rows.map(([t, w, tool], i) => (
        <div className="route-row" key={t} style={{ animation: `nodeIn 0.5s var(--ease) ${i * 80}ms both` }}>
          <span>{t}</span>
          <span className="arr"><ArrowRight /></span>
          <span style={{ textAlign: "right" }}>
            <span className={w === "Hiring request" ? "sky" : "violet"}>{w}</span>
            <span className="tiny dim mono" style={{ display: "block" }}>{tool}</span>
          </span>
        </div>
      ))}
      <div className="flowline" />
    </Frame>
  );
}

function HireVisual() {
  const steps = [
    [ScrollText, "Posted on ForeMan Jobs", "Brand Designer · $25 per task", "sky", "Live"],
    [Briefcase, "14 applied · 3 shortlisted", "Skill match on the CVs", "violet", "Done"],
    [Video, "AI interview", "Disclosed, consent recorded, 3 questions", "violet", "Scored"],
    [FileSignature, "Contract sent", "freelancer-services-v1 via DocuSeal", "lime", "Signed"],
    [BadgeCheck, "Brief portal access", "Submit work, get it accepted", "lime", "Active"],
  ];
  return (
    <Frame title="Hiring pipeline" badge={<span className="badge badge-sky">Human role</span>}>
      <div className="pipe">
        {steps.map(([Icon, a, b, tone, st], i) => (
          <div className={`pipe-step tone-${tone}`} key={a} style={{ animation: `nodeIn 0.5s var(--ease) ${i * 90}ms both` }}>
            <span className="ic"><Icon /></span>
            <span className="grow"><b style={{ fontWeight: 500 }}>{a}</b><span className="tiny dim" style={{ display: "block" }}>{b}</span></span>
            <span className={`badge badge-${tone}`}>{st}</span>
          </div>
        ))}
      </div>
    </Frame>
  );
}

function ReviewVisual({ on }) {
  const data = [
    ["Eng 1", 94], ["Eng 2", 88], ["Mkt", 91], ["Sales", 79], ["Ops", 42],
  ];
  return (
    <Frame title="Completion rate · this period" badge={<span className="badge badge-ember">1 below threshold</span>}>
      <div className="bars">
        {data.map(([n, v]) => (
          <div key={n}>
            <span className="mono" style={{ color: v < 70 ? "var(--ember)" : "var(--fog)" }}>{v}%</span>
            <i style={{ height: on ? `${v}%` : "4%", background: v < 70 ? "var(--ember)" : "var(--violet)" }} />
            <span>{n}</span>
          </div>
        ))}
      </div>
      <div className="pipe-step tone-ember" style={{ marginTop: 8 }}>
        <span className="ic"><Hourglass /></span>
        <span className="grow small">Ops Agent: second period below 70% · 12 tasks · 21 days</span>
        <span className="badge badge-ember">Fired</span>
      </div>
    </Frame>
  );
}

function PayVisual() {
  const steps = [
    [Banknote, "Payroll", "Accepted work × rate", "lime", "$5.00"],
    [Gauge, "Policy engine", "Under $50 · inside the cap", "lime", "Allow"],
    [BadgeCheck, "Signed authorization", "Attestation issued", "lime", "OK"],
    [KeyRound, "Signer → OpenBao", "Re-checked, then signed in the vault", "lime", "Signed"],
    [Landmark, "NEAR", "Transfer confirmed", "lime", "Sent"],
  ];
  return (
    <Frame title="Payout #2481" badge={<span className="badge badge-lime">Autonomous</span>}>
      <div className="pipe">
        {steps.map(([Icon, a, b, tone, st], i) => (
          <div className={`pipe-step tone-${tone}`} key={a} style={{ animation: `nodeIn 0.5s var(--ease) ${i * 110}ms both` }}>
            <span className="ic"><Icon /></span>
            <span className="grow"><b style={{ fontWeight: 500 }}>{a}</b><span className="tiny dim" style={{ display: "block" }}>{b}</span></span>
            <span className={`badge badge-${tone}`}>{st}</span>
          </div>
        ))}
      </div>
    </Frame>
  );
}
