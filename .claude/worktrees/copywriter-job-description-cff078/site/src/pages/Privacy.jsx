import { Info } from "lucide-react";
import { PageHero } from "../components/ui.jsx";
import { POLICY } from "../lib/config.js";
import { useScrollSpy } from "../lib/hooks.js";

const TOC = [
  { id: "who", label: "Who's responsible" },
  { id: "candidates", label: "Candidates" },
  { id: "interviews", label: "Interviews" },
  { id: "workers", label: "Workers and freelancers" },
  { id: "companies", label: "Companies and their keys" },
  { id: "ai", label: "AI and automated decisions" },
  { id: "security", label: "Security" },
  { id: "rights", label: "Your choices" },
];

export default function Privacy() {
  const on = useScrollSpy(TOC, "-20% 0px -70% 0px");

  return (
    <>
      <PageHero eyebrow="Privacy and data" lines={["What ForeMan keeps,", "and for how long."]} lead="A plain description of how ForeMan, and the AI that runs each company on it, handles the data of candidates, workers and the companies themselves." />
      <section className="section-tight" style={{ paddingTop: 0 }}>
        <div className="wrap with-toc">
          <nav className="toc" aria-label="Sections">
            {TOC.map((t) => (
              <a key={t.id} href={`#${t.id}`} className={on === t.id ? "on" : ""}>{t.label}</a>
            ))}
          </nav>
          <article className="prose">
            <div className="note" style={{ marginBottom: 40 }}>
              <Info aria-hidden="true" />
              <span>This page describes what the software does. Each company that runs ForeMan decides how it uses it and is responsible for the personal data it collects. Check that company's own privacy notice, which you're shown when you apply.</span>
            </div>

            <h2 id="who">Who's responsible</h2>
            <p>When you apply for a job, interview or work for a company that uses ForeMan, <strong>that company</strong> is responsible for your data. On the hosted platform, each company's data lives in <strong>its own database</strong>, separate from every other company's.</p>

            <h2 id="candidates">Candidates</h2>
            <p>When you apply on a company's jobs board, ForeMan stores what you send: your name, email, skills, your CV (PDF), and an optional portfolio link and note. You must accept a short privacy notice to apply, and each email can apply once per job.</p>
            <p>Your application is used to shortlist candidates by skill match and, if you're shortlisted, to invite you to an interview.</p>
            <p><strong>If you're not hired</strong>, your application, interview transcript and contact details are redacted after <strong>{POLICY.retentionDays} days</strong>, or sooner if the company has set a shorter period or you ask. Rubric scores are kept <strong>without your identity</strong> so the company can audit its hiring for bias.</p>

            <h2 id="interviews">Interviews</h2>
            <ul>
              <li>The interviewer tells you it is an AI before anything else.</li>
              <li>It asks for your consent to record. If you say no, the interview ends and nothing is recorded.</li>
              <li>With consent, the interview is transcribed, and a summary and rubric scores are produced from the transcript.</li>
              <li>Scoring uses only the job's rubric criteria.</li>
              <li>Speech-to-text, text-to-speech and the language model run on the providers the company connects (for example Deepgram, Cartesia and Anthropic's Claude).</li>
            </ul>

            <h2 id="workers">Workers and freelancers</h2>
            <p>For people who are hired, ForeMan keeps what's needed to work and be paid: the signed contract, task briefs and deliverables, performance records, and a tax form (W-9 or W-8BEN) handled through the company's document-signing service.</p>
            <p>Pay goes to a <strong>NEAR address you've proven you own</strong>. NEAR is a public blockchain, so the transfer itself (addresses, amount, time) is publicly visible, as with any on-chain payment.</p>
            <p>Performance is measured from results (tasks completed, deadlines met, cost and whether work passed its checks), not from monitoring you.</p>

            <h2 id="companies">Companies and their keys</h2>
            <p>Integration keys a company adds (email, contracts, CRM, AI models, tools) are <strong>encrypted at rest</strong> and never shown again; the dashboard only shows the last four characters.</p>
            <p>A company's NEAR treasury key is generated inside OpenBao and <strong>cannot be exported</strong>. ForeMan never holds customer money.</p>
            <p>Subscription payments are made on NEAR; ForeMan stores the transaction hash and the details it verified on chain. No card details are collected.</p>

            <h2 id="ai">AI and automated decisions</h2>
            <p>Companies on ForeMan are run by AI: it plans the work, drafts, shortlists, interviews, hires, reviews performance and summarises. Every decision with real effect goes through deterministic rules: hiring needs an approved contract template, firing needs a minimum record and repeated poor results, and any payout of $50 or more needs a human's approval. Text from candidates and tools is treated as data and can't instruct the system.</p>
            <p>Every decision is written to an audit log that can't be edited or deleted, so it can be reviewed later.</p>

            <h2 id="security">Security</h2>
            <p>Dashboard access needs a password and an authenticator code. Outgoing tool calls can't reach private networks. Each company's data sits in its own database, with nightly backups. More detail is on the <a className="link" href="/security/">security page</a>.</p>

            <h2 id="rights">Your choices</h2>
            <ul>
              <li>You can decline recording; the interview then simply ends.</li>
              <li>You can ask the company to delete your candidate data at any time.</li>
              <li>You can ask the company what it holds about you, and to correct it.</li>
            </ul>
            <p>Contact the company that you applied to or work for; their contact details are in the emails you receive and on their jobs board.</p>
          </article>
        </div>
      </section>
    </>
  );
}
