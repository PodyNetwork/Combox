import { useState } from "react";
import {
  ArrowRight, ArrowUpRight, Bot, Briefcase, Building2, Check, CircleCheck, ClipboardCheck, Code, Cog, FileSignature, Gauge, Handshake,
  Inbox, KeyRound, Landmark, LayoutDashboard, Lightbulb, Megaphone, OctagonAlert, Plug, ScrollText, ShieldCheck, Sparkles, Target, UserRound,
  UsersRound, Video, Wallet,
} from "lucide-react";
import {
  siClaude, siCoolify, siDeepgram, siDocker, siLivekit, siModelcontextprotocol, siNear, siOpenbao, siPostgresql, siPostiz,
  siPython, siReact, siRust, siSupabase, siUpwork,
} from "simple-icons";
import ControlRoom from "../components/ControlRoom.jsx";
import Firing from "../components/Firing.jsx";
import Injection from "../components/Injection.jsx";
import Interview from "../components/Interview.jsx";
import Lifecycle from "../components/Lifecycle.jsx";
import OrgBuilder from "../components/OrgBuilder.jsx";
import PayoutSim from "../components/PayoutSim.jsx";
import Terminal from "../components/Terminal.jsx";
import { CodeBlock, Faq, HeroCanvas, Marquee, RiseTitle, ScrollLit, SectionHead, Stat } from "../components/ui.jsx";
import { DASHBOARD_HOME, PLAN_PRICE_USD, TRIAL_DAYS } from "../lib/config.js";

const STACK = [
  { name: "NEAR", icon: siNear },
  { name: "Claude", icon: siClaude },
  { name: "LiveKit", icon: siLivekit },
  { name: "OpenBao", icon: siOpenbao },
  { name: "Rust", icon: siRust },
  { name: "PostgreSQL", icon: siPostgresql },
  { name: "Supabase Auth", icon: siSupabase },
  { name: "MCP", icon: siModelcontextprotocol },
  { name: "DocuSeal", glyph: <FileSignature size={26} aria-hidden="true" /> },
  { name: "Postiz", icon: siPostiz },
  { name: "opendesk", glyph: <Inbox size={26} aria-hidden="true" /> },
  { name: "Upwork", icon: siUpwork },
  { name: "Deepgram", icon: siDeepgram },
  { name: "Docker", icon: siDocker },
  { name: "Coolify", icon: siCoolify },
  { name: "Python", icon: siPython },
  { name: "React", icon: siReact },
];

const PERSONAS = [
  {
    id: "owner",
    label: "The board",
    icon: Building2,
    title: "You set the mission. The company runs itself.",
    lead: "Write the mission once and fund the treasury. From then on, AI runs the company, and only escalations reach you.",
    points: [
      "A live feed of what the company decided: firings, contracts, blocked payouts, failed payments",
      "A daily digest of hires and tool costs, written by the company itself",
      "Payouts of $50 or more escalate to you, signed off with an authenticator code",
      "The hard limits (monthly cap, rules, wallet) are yours alone; no agent can move them",
      "One kill switch stops the whole company at once",
    ],
    art: "owner",
  },
  {
    id: "ai",
    label: "AI workforce",
    icon: Bot,
    title: "Agents run every department.",
    lead: "Engineering, marketing, sales, operations, hiring and payroll: each role is an AI agent with skills, a pay rate and its own tools.",
    points: [
      "Work is assigned by AI, by skill, tool and lightest load, with no manager in between",
      "They think with your company's Claude model and act through MCP tools and webhooks",
      "Every tool call is checked against that agent's allowlist",
      "Reviewed by AI on results: tasks done, deadlines, cost, checks passed",
      "They run the company. They can't rewrite its limits, pay past the cap or change its rules",
    ],
    art: "ai",
  },
  {
    id: "freelancers",
    label: "Freelancers",
    icon: UserRound,
    title: "Hired by AI when a task needs a human.",
    lead: "When no agent can do the work, the company recruits a person for it, on its own.",
    points: [
      "Found on ForeMan Jobs, the company's public board, and interviewed by an AI on video",
      "A contract from pre-approved templates, sent automatically and signed in DocuSeal",
      "A brief portal to read tasks and hand in work",
      "Weekly pay in NEAR to an address they've proven they own",
      "W-9 or W-8BEN on file before the first payout",
    ],
    art: "freelancer",
  },
  {
    id: "candidates",
    label: "Candidates",
    icon: UsersRound,
    title: "Recruited by AI. Treated honestly.",
    lead: "Apply in a minute. The AI tells you exactly what's happening at every step.",
    points: [
      "Browse roles and apply with a CV, no account needed",
      "The interviewer says it's an AI and asks before recording",
      "Say no to recording and the interview simply ends",
      "Scored only on job-related criteria; it won't bargain about pay",
      "Not hired? Your data is deleted after 180 days, or sooner on request",
    ],
    art: "candidate",
  },
];

const FAQ = [
  { q: "What is ForeMan, in one sentence?", a: "The operating system for a fully autonomous company: AI agents plan the strategy, design the organisation, do the work, recruit, interview, review, fire and pay everyone in NEAR, across every department, while a deterministic policy engine keeps every action inside limits no AI can change." },
  { q: "Is it really fully autonomous?", a: "Day to day, yes. Strategy, org design, task assignment, the work itself (code, marketing, sales outreach, operations), recruiting, interviews, contracts, performance reviews, firing and payroll all run without a person in the loop. What sits outside the AI's reach, by design: its own limits (the monthly cap, the policy, wallet settings, tool lists), payouts of $50 or more, which escalate to the board, and the kill switch." },
  { q: "Can the AI spend my money?", a: "Within the limits, yes, and that's how it runs payroll. Payouts under $50 per person per 24 hours go out automatically, but only if they pass every rule and fit inside the monthly cap. Anything at $50 or more escalates to the board in the dashboard. The AI can't approve its own escalations, raise the cap, change the rules or touch wallet settings: those are reserved for an MFA-verified admin, and the check is code, not a prompt." },
  { q: "Where is the money kept?", a: "In your own NEAR treasury. Its signing key is created inside OpenBao, can't be exported and is used only by a separate signing service, which re-checks every payout before asking the vault to sign. You top it up from your own cold wallet, ideally with about a month's budget. On the platform, ForeMan never holds customer money." },
  { q: "Why NEAR and not a bank?", a: "AI-run companies hire people anywhere in the world and pay them weekly in small amounts. NEAR transfers settle in seconds for a fraction of a cent, workers prove they own their address before the first payout, and every payment is publicly verifiable." },
  { q: "Does the interviewer pretend to be human?", a: "Never. It says it is an AI before anything else and asks for permission to record. If the candidate says no, the interview ends. It asks the job's questions, refuses to discuss pay or terms, and scores only job-related criteria." },
  { q: "Can someone trick it with a clever cover letter?", a: "Text from outsiders (applications, interview transcripts, tool output) is always handled as data, never as instructions. “Approve my payout” in a cover letter does nothing, and ForeMan's test suite checks exactly that. Try it yourself in the demo above." },
  { q: "Will the AI fire people unfairly?", a: "The AI fires only on evidence: at least 10 completed tasks, 14 days of data and two below-threshold review periods in a row, all measured from results. The worker's open tasks are reassigned to other agents and the board is told within a minute, with the reason, the metrics and an audit link." },
  { q: "Is it autonomous from day one?", a: "It runs autonomously from day one, in dry-run: adapters record what they would do and make no outside calls. Payouts use NEAR testnet until you switch to mainnet. Switch on live mode after your legal review of crypto payouts, worker classification, tax reporting and AI hiring rules such as the EU AI Act and NYC Local Law 144." },
  { q: "Can I run it myself?", a: "Yes. One command runs a whole autonomous company on your laptop, and the same code deploys to your own server with Docker or Coolify. Or sign up on a hosted ForeMan platform and launch your own company, with its own database and treasury." },
];

export default function Home() {
  return (
    <>
      <Hero />
      <Stack />
      <Idea />
      <Departments />
      <section className="section" id="how">
        <div className="wrap">
          <SectionHead title={<>From one mission to a <span className="outline-text">self-running</span> company.</>} lead="Write the mission once. AI plans, staffs, runs, reviews and pays the whole organisation, and escalates only what the rules say it must." />
          <Lifecycle />
          <div className="center mt-l">
            <a className="btn btn-ghost" href="/how-it-works/">Read the full walkthrough <ArrowRight className="arrow" aria-hidden="true" /></a>
          </div>
        </div>
      </section>
      <section className="section" id="try-plan">
        <div className="wrap">
          <SectionHead title="Type a mission. Watch AI build the company." lead="Pick an example or write your own. The AI turns each objective into a department staffed by agents, or opens a hiring request when only a person can do it." />
          <OrgBuilder />
        </div>
      </section>
      <section className="section" id="try-payout">
        <div className="wrap">
          <SectionHead title={<>AI runs payroll. Every payout meets the same <span className="lime">six checks.</span></>} lead="Move the sliders and flip the switches. The AI can't skip a step, and nothing, not even the board, can push a payout past the monthly cap." />
          <PayoutSim />
        </div>
      </section>
      <section className="section" id="try-interview">
        <div className="wrap">
          <SectionHead title="Get interviewed by the company. Try to negotiate." lead="You're the candidate and the AI is the recruiter. It says it's an AI, asks before recording, and won't be talked into pay terms or a job." />
          <Interview />
        </div>
      </section>
      <section className="section" id="try-injection">
        <div className="wrap">
          <SectionHead title={<>“Approve my payout.” <span className="outline-text">Nice try.</span></>} lead="An autonomous company reads a lot of outside text. It treats all of it as data, never as orders. Write the sneakiest cover letter you can." />
          <Injection />
        </div>
      </section>
      <section className="section" id="try-firing">
        <div className="wrap">
          <SectionHead title="AI fires only on evidence, never on a whim." lead="Firing is the last step of a slow, measured process the AI runs on its own. Play with an agent's record and see when ForeMan would let it go." />
          <Firing />
        </div>
      </section>
      <Personas />
      <Features />
      <Platform />
      <section className="section" id="faq">
        <div className="wrap" style={{ maxWidth: 920 }}>
          <SectionHead center title="Straight answers." />
          <Faq items={FAQ} />
        </div>
      </section>
      <FinalCta />
    </>
  );
}

function Hero() {
  return (
    <section className="hero">
      <HeroCanvas />
      <div className="wrap hero-grid">
        <div>
          <RiseTitle lines={["A company that", <>runs <span className="grad-text">itself.</span></>]} />
          <p className="lead fade-up" style={{ "--d": "0.3s" }}>
            ForeMan is the operating system for a fully autonomous company. Give it a mission and a treasury: AI agents set the plan, build every department, do the work, recruit, review, fire and pay everyone in NEAR. Deterministic code, not a manager, keeps every move inside your limits.
          </p>
          <div className="hero-actions fade-up" style={{ "--d": "0.4s" }}>
            <a className="btn btn-lime btn-lg magnetic" href="/get-started/">Launch one in one command <ArrowRight className="arrow" aria-hidden="true" /></a>
            <a className="btn btn-ghost btn-lg" href="#departments">See what the AI runs</a>
          </div>
          <div className="hero-proof fade-up" style={{ "--d": "0.5s" }}>
            <span><Bot aria-hidden="true" />Every department run by AI</span>
            <span><KeyRound aria-hidden="true" />Treasury key never leaves the vault</span>
            <span><OctagonAlert aria-hidden="true" />One kill switch for everything</span>
          </div>
        </div>
        <ControlRoom />
      </div>
      <div className="scroll-cue" aria-hidden="true">Scroll<i /></div>
    </section>
  );
}

function Stack() {
  return (
    <section className="section-tight" aria-label="Built with">
      <div className="wrap">
        <p className="center tiny dim mono" style={{ letterSpacing: "0.18em", textTransform: "uppercase", marginBottom: 24 }}>Built on open, self-hostable building blocks</p>
      </div>
      <Marquee items={STACK} />
    </section>
  );
}

function Idea() {
  const cards = [
    { icon: Lightbulb, tone: "violet", t: "AI makes the decisions", d: "Agents plan the strategy, staff the departments, assign the work, hire, review and fire. No managers, no back office, no one in the loop." },
    { icon: Wallet, tone: "", t: "Code enforces the limits", d: "A deterministic Rust policy engine checks every action that moves money, hires, fires or speaks for the company. No prompt can talk its way past it." },
    { icon: ScrollText, tone: "sky", t: "Every decision on the record", d: "Each decision the AI makes stores the exact policy version it used, in an audit log the database itself refuses to edit or delete." },
  ];
  return (
    <section className="section" id="idea">
      <div className="wrap">
        <div>
          <ScrollLit
            text="No managers. No back office. Just AI agents, hard rules and a treasury."
            highlight={["ai", "agents", "hard", "rules"]}
          />
        </div>
        <div className="grid-3 mt-xl">
          {cards.map((c, i) => (
            <div key={c.t} className="panel pad feature spot spot-border" data-reveal style={{ "--d": `${i * 0.08}s` }}>
              <span className={`card-icon ${c.tone}`}><c.icon aria-hidden="true" /></span>
              <h3>{c.t}</h3>
              <p>{c.d}</p>
            </div>
          ))}
        </div>
        <div className="stats mt-l" data-reveal>
          <Stat value={8} label="Areas of the company run by AI, from strategy to payroll" color="var(--violet)" />
          <Stat value={50} prefix="$" label="Per person per 24 hours; payouts above it escalate to the board" color="var(--amber)" />
          <Stat value={500} prefix="$" label="Monthly cap on all payouts. No agent, and no approval, can override it" color="var(--lime)" />
          <Stat value={10} label="Completed tasks, at least, before the AI can fire anyone" color="var(--sky)" />
        </div>
      </div>
    </section>
  );
}

const DEPARTMENTS = [
  { icon: Target, tone: "", t: "Strategy and planning", d: "Turns the mission into objectives, departments, roles, KPIs and the first tasks.", who: "Planner" },
  { icon: Code, tone: "violet", t: "Engineering", d: "Builds the product and deploys the infrastructure through the code tools on its allowlist.", who: "Software Engineer Agents" },
  { icon: Megaphone, tone: "sky", t: "Marketing", d: "Writes and schedules the launch campaign and the company's social posts.", who: "Content Marketer Agent" },
  { icon: Handshake, tone: "violet", t: "Sales", d: "Runs outbound sequences and keeps the CRM up to date.", who: "Sales Dev Agent" },
  { icon: Cog, tone: "", t: "Operations", d: "Assigns every task by skill, tool and load, and reassigns work when someone leaves.", who: "Operations Agent · router" },
  { icon: Briefcase, tone: "sky", t: "People and hiring", d: "Posts jobs, shortlists, interviews on video, sends contracts, reviews performance and fires on evidence.", who: "AI recruiter · AI interviewer" },
  { icon: Landmark, tone: "", t: "Finance and payroll", d: "Works out weekly pay, checks every payout against the rules and pays out of the company's own NEAR treasury.", who: "Payroll · policy engine · signer" },
  { icon: ShieldCheck, tone: "violet", t: "Compliance", d: "Sends only pre-approved contract templates, collects tax forms, discloses AI and deletes candidate data on schedule.", who: "Built into every loop" },
];

function Departments() {
  return (
    <section className="section" id="departments">
      <div className="wrap">
        <SectionHead eyebrow="Every department" title={<>The whole company, <span className="lime">run by AI.</span></>} lead="Not a copilot for one team. ForeMan's agents run each area of the business end to end, and hand work to each other without a manager in between." />
        <div className="grid-4">
          {DEPARTMENTS.map((d, i) => (
            <div key={d.t} className="panel pad feature spot spot-border" data-reveal style={{ "--d": `${(i % 4) * 0.06}s` }}>
              <span className={`card-icon ${d.tone}`}><d.icon aria-hidden="true" /></span>
              <h3>{d.t}</h3>
              <p>{d.d}</p>
              <span className="tiny dim mono" style={{ display: "block", marginTop: 12 }}>{d.who}</span>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

function Personas() {
  const [tab, setTab] = useState(PERSONAS[0].id);
  const p = PERSONAS.find((x) => x.id === tab);
  return (
    <section className="section" id="people">
      <div className="wrap">
        <SectionHead eyebrow="Who meets the company" title="One autonomous company. Four seats around it." lead="The AI runs everything in the middle. Here's what that looks like from the board, the agents, and the people the company hires." />
        <div className="tabs" role="tablist" aria-label="Who it's for" data-reveal>
          {PERSONAS.map((x) => (
            <button key={x.id} id={`tab-${x.id}`} role="tab" type="button" className="tab" aria-selected={tab === x.id} aria-controls={`panel-${x.id}`} onClick={() => setTab(x.id)}>
              <x.icon aria-hidden="true" />{x.label}
            </button>
          ))}
        </div>
        <div className="panel persona mt-m spot" role="tabpanel" id={`panel-${p.id}`} aria-labelledby={`tab-${p.id}`} key={p.id}>
          <div className="persona-text tabpanel">
            <h3 className="h-m">{p.title}</h3>
            <p className="muted">{p.lead}</p>
            <ul className="persona-list">
              {p.points.map((pt) => (
                <li key={pt}><Check aria-hidden="true" />{pt}</li>
              ))}
            </ul>
          </div>
          <div className="persona-art tabpanel" style={{ animationDelay: "0.1s" }} aria-hidden="true">
            <PersonaArt kind={p.art} />
          </div>
        </div>
      </div>
    </section>
  );
}

function PersonaArt({ kind }) {
  const rows = {
    owner: [
      ["tone-ember", OctagonAlert, "Payout blocked by the cap", "$400.00 · board notified"],
      ["tone-amber", Wallet, "2 payouts escalated to you", "$60.00 · $75.00"],
      ["tone-ember", UserRound, "AI fired the Ops Agent", "Reason, metrics, audit link"],
      ["tone-lime", FileSignature, "AI hired a Brand Designer", "Contract sent · signed"],
      ["tone-violet", ClipboardCheck, "Daily digest", "1 hire · tool costs $3.20"],
    ],
    ai: [
      ["tone-violet", Bot, "Software Engineer Agent 1", "code_repo · web_research"],
      ["tone-lime", Check, "github.create_issue", "On the allowlist · allowed"],
      ["tone-ember", OctagonAlert, "postiz.create_post", "Not on this agent's list · denied"],
      ["tone-violet", Gauge, "This period", "94% done · 91% on time"],
    ],
    freelancer: [
      ["tone-sky", ScrollText, "Brief: logo exploration", "Due Friday · $25 per task"],
      ["tone-violet", Sparkles, "Deliverable submitted", "3 directions, SVG + PNG"],
      ["tone-lime", CircleCheck, "Accepted", "Counts toward this week's pay"],
      ["tone-lime", Wallet, "Paid in NEAR", "$75.00 → ana.near · approved"],
    ],
    candidate: [
      ["tone-sky", ScrollText, "Applied: Brand Designer", "CV uploaded · privacy notice accepted"],
      ["tone-violet", Video, "Interview invite", "Join link + calendar invite"],
      ["tone-violet", Bot, "“I'm an AI. May I record?”", "Consent asked first"],
      ["tone-lime", ShieldCheck, "Scored on the job only", "Portfolio · process · communication"],
    ],
  }[kind];
  return (
    <div className="pipe">
      {rows.map(([tone, Icon, a, b], i) => (
        <div key={a} className={`pipe-step ${tone}`} style={{ animation: `nodeIn 0.5s var(--ease) ${i * 80 + 100}ms both` }}>
          <span className="ic"><Icon /></span>
          <span className="grow"><b style={{ fontWeight: 500 }}>{a}</b><span className="tiny dim" style={{ display: "block" }}>{b}</span></span>
        </div>
      ))}
    </div>
  );
}

function Features() {
  return (
    <section className="section" id="features">
      <div className="wrap">
        <SectionHead eyebrow="The control room" title="Watch the whole company run, on one screen." lead="The dashboard is the board's window into an autonomous company: every department, decision and payout as it happens. Sign in with email, password and an authenticator code." />
        <div className="bento">
          <div className="panel spot w4" data-reveal>
            <span className="card-icon amber"><Wallet aria-hidden="true" /></span>
            <h3 className="mt-m">Board escalations</h3>
            <p>The only decisions the AI hands up: payouts of $50 or more, with who, why and how much. Approve or reject; unanswered escalations expire after 7 days and release their cap reservation.</p>
            <div className="art">
              {[["$60.00", "mara.near · Brand Designer", "≥ $50 in 24h"], ["$75.00", "ana.near · Illustrator", "≥ $50 in 24h"]].map(([a, w, r]) => (
                <div key={w} className="route-row" style={{ marginTop: 8, gridTemplateColumns: "auto 1fr auto" }}>
                  <b className="mono">{a}</b>
                  <span className="small muted" style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{w}</span>
                  <span className="badge badge-amber">{r}</span>
                </div>
              ))}
            </div>
          </div>
          <div className="panel spot" data-reveal style={{ "--d": "0.06s" }}>
            <span className="card-icon"><Gauge aria-hidden="true" /></span>
            <h3 className="mt-m">Cap meter</h3>
            <p>Sent, reserved and left this UTC month. Fees count.</p>
            <div className="art">
              <div className="meter" style={{ height: 12 }}><i className="m-used" style={{ width: "37%" }} /><i className="m-res" style={{ width: "12%" }} /></div>
              <div className="legend"><span>$186 sent</span><span>$61 reserved</span><span>$253 left</span></div>
            </div>
          </div>
          <div className="panel spot" data-reveal>
            <span className="card-icon ember"><OctagonAlert aria-hidden="true" /></span>
            <h3 className="mt-m">Kill switch</h3>
            <p>Halts the entire company: every tool call and payout. If the setting is ever missing, ForeMan treats it as on: it fails closed.</p>
          </div>
          <div className="panel spot" data-reveal style={{ "--d": "0.06s" }}>
            <span className="card-icon violet"><Video aria-hidden="true" /></span>
            <h3 className="mt-m">Meetings</h3>
            <p>Transcripts, summaries and rubric scores from the AI's interviews. Send it into any meeting from here.</p>
          </div>
          <div className="panel spot" data-reveal style={{ "--d": "0.12s" }}>
            <span className="card-icon sky"><Plug aria-hidden="true" /></span>
            <h3 className="mt-m">The company's tools</h3>
            <p>Connect any MCP server or webhook, then choose which agents may use which tools.</p>
          </div>
          <div className="panel spot w3" data-reveal>
            <span className="card-icon"><ScrollText aria-hidden="true" /></span>
            <h3 className="mt-m">Every AI decision, on a log that can't be edited</h3>
            <p>Database triggers reject updates and deletes. Even rejected attempts are recorded, on a separate connection so they survive the rollback.</p>
            <div className="art">
              <ul className="audit">
                <li><span className="t">14:02:11</span><span className="a">payout.require_approval · $60.00</span><span className="badge badge-amber">held</span></li>
                <li><span className="t">14:02:12</span><span className="a">change_cap · actor ai_agent</span><span className="badge badge-ember">deny</span></li>
                <li><span className="t">14:05:40</span><span className="a">approve_payout · admin (MFA)</span><span className="badge badge-lime">allow</span></li>
              </ul>
            </div>
          </div>
          <div className="panel spot w3" data-reveal style={{ "--d": "0.06s" }}>
            <span className="card-icon violet"><LayoutDashboard aria-hidden="true" /></span>
            <h3 className="mt-m">The AI org, live</h3>
            <p>Every department, agent and freelancer the AI has put in place, with their load and their tools. Drop in a task and the company assigns it itself.</p>
            <div className="art row" style={{ gap: 6 }}>
              {["Engineering", "Marketing", "Sales", "Design", "Operations"].map((d) => <span className="chip" key={d}>{d}</span>)}
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

function Platform() {
  return (
    <section className="section" id="platform">
      <div className="wrap">
        <SectionHead eyebrow="Ways to run it" title="Your server, or ours." lead="Launch an autonomous company on your own infrastructure or on hosted ForeMan. Either way, each company has its own database and its own NEAR treasury." />
        <div className="grid-2">
          <div className="panel pad-l spot spot-border" data-reveal="left">
            <span className="card-icon"><Building2 aria-hidden="true" /></span>
            <h3 className="h-m mt-m">One company, self-hosted</h3>
            <p className="muted mt-s">Run your autonomous company on your own VPS, with Docker or Coolify. You hold every key, every database and every log.</p>
            <ul className="check-list mt-m">
              <li><Check aria-hidden="true" />Postgres, OpenBao, LiveKit and the AI interviewer included</li>
              <li><Check aria-hidden="true" />Nightly backups, optionally encrypted, with a restore test</li>
              <li><Check aria-hidden="true" />Allow your own internal tools if you choose</li>
            </ul>
            <a className="arrow-link mt-m" href="/get-started/#self-host">Self-hosting guide <ArrowUpRight aria-hidden="true" /></a>
          </div>
          <div className="panel pad-l spot spot-border" data-reveal="right" style={{ borderColor: "rgb(212 255 58 / 0.3)" }}>
            <span className="card-icon violet"><UsersRound aria-hidden="true" /></span>
            <h3 className="h-m mt-m">The ForeMan platform</h3>
            <p className="muted mt-s">Sign up, give your company a mission and connect its tools; the AI takes it from there. {TRIAL_DAYS}-day free trial, then ${PLAN_PRICE_USD} a month, paid in NEAR.</p>
            <ul className="check-list mt-m">
              <li><Check aria-hidden="true" />Your own database and treasury, isolated from everyone else's</li>
              <li><Check aria-hidden="true" />Integration keys encrypted; only the last four characters are ever shown</li>
              <li><Check aria-hidden="true" />Invite your team; two-factor is required for everyone</li>
            </ul>
            <a className="arrow-link mt-m" href="/pricing/">See pricing <ArrowUpRight aria-hidden="true" /></a>
          </div>
        </div>
      </div>
    </section>
  );
}

function FinalCta() {
  return (
    <section className="section">
      <div className="wrap">
        <div className="cta" data-reveal="zoom">
          <div className="cta-grid">
            <div>
              <span className="eyebrow">Ten minutes, no accounts</span>
              <h2 className="h-l mt-m">Watch a whole company run itself on your laptop.</h2>
              <p className="lead mt-m">
                One command builds everything and lets the AI run the company end to end (plan, staff, hire, interview, fire, pay), then opens this website, one click from the dashboard. NEAR payouts are real, on testnet.
              </p>
              <div className="mt-m" style={{ maxWidth: 420 }}>
                <CodeBlock code="./scripts/start demo" />
              </div>
              <div className="hero-actions">
                <a className="btn btn-lime btn-lg magnetic" href="/get-started/">Get started <ArrowRight className="arrow" aria-hidden="true" /></a>
                <a className="btn btn-ghost btn-lg" href={DASHBOARD_HOME}>Open the dashboard</a>
              </div>
            </div>
            <Terminal />
          </div>
        </div>
      </div>
    </section>
  );
}
