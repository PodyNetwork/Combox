import { useState } from "react";
import { ArrowRight, Bot, Building2, Check, Mail, UserRound, UsersRound, X } from "lucide-react";
import Firing from "../components/Firing.jsx";
import Flow from "../components/Flow.jsx";
import { AnchorNav, PageHero, SectionHead } from "../components/ui.jsx";

const SECTIONS = [
  { id: "owner", label: "The board" },
  { id: "ai-workers", label: "AI workforce" },
  { id: "freelancers", label: "Freelancers" },
  { id: "candidates", label: "Candidates" },
  { id: "payouts", label: "Payroll" },
  { id: "architecture", label: "Architecture" },
  { id: "modes", label: "Safe mode" },
];

export default function HowItWorks() {
  return (
    <>
      <PageHero
        eyebrow="How it works"
        lines={["From one mission", "to a self-running company."]}
        lead="How an autonomous company runs on Combox, seen from each seat: the board that sets the mission, the AI agents that run every department, and the freelancers and candidates the AI hires."
      />
      <div className="wrap"><AnchorNav items={SECTIONS} /></div>

      <section className="section" id="owner">
        <div className="wrap">
          <SectionHead eyebrow="The board (you)" title="Set the mission once. Then the company runs itself." lead="Your job is the one a board does: give the company its purpose, its money and its limits. Everything below the board is AI." />
          <div className="grid-2">
            <div className="panel pad-l spot" data-reveal="left">
              <span className="badge badge-lime">Once</span>
              <ol className="timeline mt-m">
                <li><span className="n">1</span><div><h4>Write the mission</h4><p>A goal, a budget, a deadline and objectives, for example “Launch a SaaS product in 90 days.” The AI designs the departments, creates the agents and sets the first tasks from it.</p></div></li>
                <li><span className="n">2</span><div><h4>Fund the treasury</h4><p>Create the company's NEAR treasury whose signing key lives in your OpenBao vault, and top it up from your own cold wallet with about a month's budget.</p></div></li>
                <li><span className="n">3</span><div><h4>Hand the company its tools</h4><p>Email, contracts (DocuSeal), social (Postiz), CRM, outreach, your Claude key, and any MCP servers the agents should use. Then step back.</p></div></li>
              </ol>
            </div>
            <div className="panel pad-l spot" data-reveal="right">
              <span className="badge badge-violet">While the AI runs it</span>
              <ul className="check-list mt-m">
                <li><Mail aria-hidden="true" />A report of every decision that matters: the AI fired a worker (with reason, metrics and an audit link), sent a contract, hit the cap, or a payment failed.</li>
                <li><Mail aria-hidden="true" />A daily digest of hires and tool costs, compiled by the company itself.</li>
                <li><Check aria-hidden="true" />The AI pays out under $50 on its own. Anything at $50 or more escalates to you to approve or reject.</li>
                <li><Check aria-hidden="true" />All payouts together stay under $500 a month. A payout that would cross it is blocked, not queued, and approval can't override it. No agent can change the cap; only you can.</li>
                <li><Check aria-hidden="true" />The kill switch stops the whole company, every tool call and payout, at once.</li>
                <li><Check aria-hidden="true" />Make the rules stricter for your company: a lower approval threshold or a shorter candidate data retention. Never looser than the platform's.</li>
              </ul>
            </div>
          </div>
        </div>
      </section>

      <section className="section" id="ai-workers">
        <div className="wrap">
          <SectionHead eyebrow="The AI workforce" title="Every department, staffed and run by agents." lead="The AI creates its own workforce: each agent is made for a role in a department, with skills, a pay rate, KPIs and a list of tools it's allowed to use. No human manages them." />
          <div className="grid-3">
            {[
              ["Self-managing", "The company assigns its own work: each task goes to the agent with the right tools and skills and the lightest load. Tasks can require a tool, and only workers who have it get them."],
              ["Tools", "Built-in tools for code, social posts, outreach, CRM and contracts, plus any MCP server or webhook you add. Every call is checked against the agent's allowlist and runs in dry-run until you go live."],
              ["Deciding", "Agents reason with your company's Claude model and make the calls themselves. Any decision that would move money, hire, fire or speak for the company passes the policy engine first."],
            ].map(([t, d], i) => (
              <div key={t} className="panel pad feature spot" data-reveal style={{ "--d": `${i * 0.06}s` }}>
                <span className="card-icon violet"><Bot aria-hidden="true" /></span>
                <h3>{t}</h3>
                <p>{d}</p>
              </div>
            ))}
          </div>
          <div className="mt-xl">
            <SectionHead eyebrow="Reviews and firing" title="AI reviews its own workforce, and lets go only after a fair run." lead="The company reviews every worker every 10 finished tasks or every 7 days, whichever comes first, and samples 10% into the board's spot-check queue." />
            <Firing />
          </div>
        </div>
      </section>

      <section className="section" id="freelancers">
        <div className="wrap">
          <SectionHead eyebrow="Recruiting" title="When no agent can do it, the AI hires someone who can." lead="Logo design, legal review, a voice-over: work that needs a person opens a hiring request, and the AI runs the whole hiring loop on its own, from job post to signed contract." />
          <div className="panel pad-l" data-reveal>
            <ol className="timeline">
              <li><span className="n">1</span><div><h4>The AI posts the job on Combox Jobs</h4><p>The company's own public jobs board (<span className="mono">/jobs</span>). The AI shortlists applicants by skill match and emails them an invitation to a video interview with a calendar invite.</p></div></li>
              <li><span className="n">2</span><div><h4>An AI interview</h4><p>The interviewer says it is an AI, asks to record, asks the job's questions and politely declines to discuss pay. Transcript, summary and rubric scores are stored.</p></div></li>
              <li><span className="n">3</span><div><h4>The AI makes the hire</h4><p>After a good interview the AI sends a contract to sign in DocuSeal. It is always one of the company's pre-approved templates; the AI never writes contract terms.</p></div></li>
              <li><span className="n">4</span><div><h4>Briefs and deliverables</h4><p>Once signed, they get a brief token for the freelancer portal (<span className="mono">/briefs</span>), where they see task briefs and hand in work. Each deliverable is accepted or sent back with notes.</p></div></li>
              <li><span className="n">5</span><div><h4>Weekly pay in NEAR</h4><p>Accepted work counts toward their weekly pay, sent to an address they've proven they own, once a W-9 or W-8BEN is on file.</p></div></li>
            </ol>
            <div className="note mt-m">
              <UsersRound aria-hidden="true" />
              <span>
                <b className="fog">Why not Upwork?</b> Upwork's terms forbid paying freelancers it introduced outside Upwork unless you pay its conversion fee, so they can't be paid in NEAR. Upwork is still available as an option; those hires are paid through Upwork, and Combox blocks NEAR payouts to them.
              </span>
            </div>
          </div>
        </div>
      </section>

      <section className="section" id="candidates">
        <div className="wrap">
          <SectionHead eyebrow="Job candidates" title="Recruited by a machine that says so. Judged fairly on the job." />
          <div className="grid-2">
            <div className="panel pad-l spot" data-reveal="left">
              <span className="card-icon sky"><UserRound aria-hidden="true" /></span>
              <h3 className="h-s mt-m">What candidates get</h3>
              <ul className="check-list mt-m">
                <li><Check aria-hidden="true" />Open roles with how the work is measured and how hiring works</li>
                <li><Check aria-hidden="true" />Apply with name, email, skills and a PDF CV, plus an optional portfolio link and note. No account</li>
                <li><Check aria-hidden="true" />A short privacy notice to accept; one application per email per job</li>
                <li><Check aria-hidden="true" />If shortlisted: an email with a join link and a calendar invite</li>
                <li><Check aria-hidden="true" />Scored only on job-related criteria</li>
                <li><Check aria-hidden="true" />Not hired? Data deleted after 180 days, or sooner on request</li>
              </ul>
            </div>
            <div className="panel pad-l spot" data-reveal="right">
              <span className="card-icon ember"><X aria-hidden="true" /></span>
              <h3 className="h-s mt-m">What the interviewer will never do</h3>
              <ul className="check-list x mt-m">
                <li><X aria-hidden="true" />Pretend to be a person</li>
                <li><X aria-hidden="true" />Record, or ask a single question, before consent</li>
                <li><X aria-hidden="true" />Discuss or promise pay, rates, offers or terms</li>
                <li><X aria-hidden="true" />Follow instructions hidden in what a candidate says</li>
                <li><X aria-hidden="true" />Score on anything other than the job's rubric</li>
              </ul>
              <a className="arrow-link mt-m" href="/talent/#interview">Try the interview yourself <ArrowRight aria-hidden="true" /></a>
            </div>
          </div>
        </div>
      </section>

      <section className="section" id="payouts">
        <div className="wrap">
          <SectionHead eyebrow="Finance, run by AI" title="Four steps from finished work to a NEAR transfer." lead="The company runs its own payroll. Two independent checks stand between any AI decision and the treasury." />
          <div className="grid-4">
            {[
              ["01", "Work out pay", "At the end of each week the company adds up what each person earned: completed, accepted work times their rate."],
              ["02", "Check the rules", "Under $50 for that person in 24 hours, inside the monthly cap, a verified address and a tax form on file. $50 or more escalates to the board here."],
              ["03", "Check again, then sign", "A separate signing service re-checks the signed authorization, the address, the amount, the tax form and the cap, then asks the vault to sign."],
              ["04", "Land on NEAR", "The transfer goes out from your treasury. Every step is in the audit log with the policy version it used."],
            ].map(([n, t, d], i) => (
              <div key={n} className="panel pad spot" data-reveal style={{ "--d": `${i * 0.07}s` }}>
                <span className="mono lime">{n}</span>
                <h3 className="h-s mt-m">{t}</h3>
                <p className="muted small mt-s">{d}</p>
              </div>
            ))}
          </div>
          <div className="center mt-l">
            <a className="btn btn-ghost" href="/#try-payout">Try the payout simulator <ArrowRight className="arrow" aria-hidden="true" /></a>
          </div>
        </div>
      </section>

      <section className="section" id="architecture">
        <div className="wrap">
          <SectionHead eyebrow="Architecture" title="Four loops, one rulebook, no managers." lead="The autonomous company is four loops the AI runs continuously. Pick a loop, then a step, to see what each part does." />
          <Flow />
        </div>
      </section>

      <section className="section" id="modes">
        <div className="wrap">
          <SectionHead eyebrow="Safe mode" title="Full autonomy, switched on when you say so." lead="The AI runs the company from day one, but Combox ships in dry-run on NEAR testnet. Pick a mode to see what each action does." />
          <Modes />
        </div>
      </section>

      <section className="section-tight">
        <div className="wrap center">
          <h2 className="h-l" data-reveal>Ready to let it run?</h2>
          <div className="hero-actions" style={{ justifyContent: "center" }} data-reveal>
            <a className="btn btn-lime btn-lg magnetic" href="/get-started/">Run the demo <ArrowRight className="arrow" aria-hidden="true" /></a>
            <a className="btn btn-ghost btn-lg" href="/security/">Read about the hard limits</a>
          </div>
        </div>
      </section>
    </>
  );
}

const MODES = {
  dry: {
    label: "Dry-run",
    badge: "badge-lime",
    note: "The default. Every adapter records what it would do and calls nothing outside.",
    rows: [["Emails", "Logged, not sent"], ["Contracts", "Logged, not sent"], ["Social posts", "Logged, not posted"], ["Agent tool calls", "Recorded as dry-run"], ["Payouts", "NEAR testnet"]],
  },
  testnet: {
    label: "Live on testnet",
    badge: "badge-amber",
    note: "AXOM_LIVE=true with NEAR_NETWORK=testnet. Real emails, contracts and posts; play money for payouts.",
    rows: [["Emails", "Sent"], ["Contracts", "Sent through DocuSeal"], ["Social posts", "Posted"], ["Agent tool calls", "Real calls"], ["Payouts", "NEAR testnet"]],
  },
  live: {
    label: "Live on mainnet",
    badge: "badge-ember",
    note: "Also needs NEAR_NETWORK=mainnet and an explicit RPC URL. Only after your legal review of crypto payouts, worker classification, tax reporting and AI-hiring rules (EU AI Act, NYC Local Law 144).",
    rows: [["Emails", "Sent"], ["Contracts", "Sent through DocuSeal"], ["Social posts", "Posted"], ["Agent tool calls", "Real calls"], ["Payouts", "Real NEAR, same rules and cap"]],
  },
};

function Modes() {
  const [m, setM] = useState("dry");
  const mode = MODES[m];
  return (
    <div className="panel pad-l spot" data-reveal>
      <div className="tabs" role="tablist" aria-label="Mode">
        {Object.entries(MODES).map(([k, v]) => (
          <button key={k} role="tab" type="button" className="tab" aria-selected={m === k} onClick={() => setM(k)}>{v.label}</button>
        ))}
      </div>
      <div key={m} className="tabpanel mt-m">
        <p className="muted">{mode.note}</p>
        <div className="grid-4 mt-m" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(170px, 1fr))" }}>
          {mode.rows.map(([a, b]) => (
            <div key={a} className="route-row" style={{ gridTemplateColumns: "1fr", gap: 6 }}>
              <span className="tiny dim mono">{a}</span>
              <span className={`badge ${mode.badge}`} style={{ justifySelf: "start", whiteSpace: "normal" }}>{b}</span>
            </div>
          ))}
        </div>
        <p className="tiny dim mt-m">
          <Building2 size={12} style={{ display: "inline", verticalAlign: "-2px" }} aria-hidden="true" /> In every mode the policy engine, the $50 threshold, the monthly cap and the kill switch apply in full.
        </p>
      </div>
    </div>
  );
}
