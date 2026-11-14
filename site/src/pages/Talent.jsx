import { ArrowRight, ArrowUpRight, BadgeCheck, CalendarCheck, FileSignature, FileText, Inbox, ScrollText, ShieldCheck, Wallet } from "lucide-react";
import { siNear } from "simple-icons";
import Interview from "../components/Interview.jsx";
import { AnchorNav, BrandIcon, Faq, PageHero, SectionHead } from "../components/ui.jsx";
import { BRIEFS_URL, JOBS_URL } from "../lib/config.js";

const SECTIONS = [
  { id: "apply", label: "Apply" },
  { id: "interview", label: "Interview" },
  { id: "contract", label: "Contract" },
  { id: "briefs", label: "Briefs" },
  { id: "pay", label: "Pay" },
  { id: "your-data", label: "Your data" },
];

const FAQ = [
  { q: "Do I need an account to apply?", a: "No. You apply with your name, email, skills and a PDF CV, plus an optional portfolio link and note. One application per email per job." },
  { q: "Will a person ever look at my application?", a: "The company is run by AI, so shortlisting, the interview and the hiring decision are made by AI. Its human board can see every application, interview transcript and score in the dashboard and is notified whenever a contract goes out, and every decision the AI makes is recorded in an audit log." },
  { q: "Why won't the interviewer discuss pay?", a: "On purpose. An AI shouldn't make commitments on a company's behalf, so pay, rates and terms are left to the written contract, which always comes from a template the company's board has approved." },
  { q: "I don't have a NEAR wallet. Can I still work?", a: "You'll need one to be paid: NEAR wallets are free to create. Before your first payout you prove you own the address, so money can only ever go to you." },
  { q: "How often am I paid?", a: "Weekly. At the end of each week, your accepted work times your rate is added up and paid in NEAR. Payouts of $50 or more are escalated to the company's board for sign-off, usually a short delay." },
  { q: "Which tax form do I need?", a: "A W-9 if you're a US person, or a W-8BEN if you're not. You sign it through the same secure document flow as your contract, and it must be on file before your first payout." },
  { q: "Can I ask for my data to be deleted?", a: "Yes, any time. If you're not hired, your application, transcript and contact details are deleted after 180 days anyway, or sooner if the company has set a shorter period." },
];

export default function Talent() {
  return (
    <>
      <PageHero eyebrow="For freelancers and candidates" lines={["Hired by AI.", "Treated fairly.", "Paid in NEAR."]} lead="Companies on Combox are run entirely by AI. When their agents need a person, they recruit one. Here's what happens when you apply, and what you can count on.">
        <div className="row">
          <a className="btn btn-lime btn-lg magnetic" href={JOBS_URL}>Browse open roles <ArrowRight className="arrow" aria-hidden="true" /></a>
          <a className="btn btn-ghost btn-lg" href={BRIEFS_URL}>Freelancer portal</a>
        </div>
      </PageHero>
      <div className="wrap"><AnchorNav items={SECTIONS} /></div>

      <section className="section" id="apply">
        <div className="wrap">
          <SectionHead eyebrow="1 · Apply" title="Two minutes. No account." lead="Every role says what the work is, how it's measured and how hiring works, before you apply." />
          <div className="grid-3">
            {[
              [FileText, "Send your CV", "Name, email, skills and a PDF CV. Add a portfolio link and a short note if you like."],
              [ShieldCheck, "Read the privacy notice", "Short and plain: what's kept, why, and for how long. Accept it to apply."],
              [Inbox, "Hear back by email", "If you're shortlisted, you get a join link for a video interview and a calendar invite."],
            ].map(([Icon, t, d], i) => (
              <div key={t} className="panel pad feature spot" data-reveal style={{ "--d": `${i * 0.06}s` }}>
                <span className="card-icon sky"><Icon aria-hidden="true" /></span>
                <h3>{t}</h3>
                <p>{d}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      <section className="section" id="interview">
        <div className="wrap">
          <SectionHead eyebrow="2 · Interview" title="Meet the interviewer before the real thing." lead="Your recruiter is the company's AI, and it tells you so. It asks before recording, sticks to the job's questions, and won't discuss pay. Have a practice run." />
          <Interview />
        </div>
      </section>

      <section className="section" id="contract">
        <div className="wrap grid-2" style={{ alignItems: "center" }}>
          <div>
            <SectionHead eyebrow="3 · Contract" title="Terms written by people, not a model." lead="After a good interview the AI sends you a contract to sign online through DocuSeal. It's always one of the company's pre-approved templates; the AI is never allowed to write or change contract terms." />
          </div>
          <div className="panel pad-l spot" data-reveal="right">
            <div className="pipe">
              {[
                [FileSignature, "freelancer-services-v1", "Board-approved template", "lime", "Sent"],
                [CalendarCheck, "Signed by you", "From any device", "lime", "Signed"],
                [FileText, "W-9 / W-8BEN", "Needed before your first payout", "amber", "Next"],
              ].map(([Icon, a, b, tone, st]) => (
                <div key={a} className={`pipe-step tone-${tone}`}>
                  <span className="ic"><Icon aria-hidden="true" /></span>
                  <span className="grow"><b className="mono" style={{ fontWeight: 500 }}>{a}</b><span className="tiny dim" style={{ display: "block" }}>{b}</span></span>
                  <span className={`badge badge-${tone}`}>{st}</span>
                </div>
              ))}
            </div>
          </div>
        </div>
      </section>

      <section className="section" id="briefs">
        <div className="wrap grid-2" style={{ alignItems: "center" }}>
          <div className="panel pad-l spot" data-reveal="left">
            <div className="pipe">
              {[
                [ScrollText, "Brief: logo exploration", "Three directions · due Friday", "sky", "Open"],
                [Inbox, "Your submission", "SVG + PNG + a short note", "violet", "Submitted"],
                [BadgeCheck, "Reviewed", "Accepted, counts toward this week", "lime", "Accepted"],
              ].map(([Icon, a, b, tone, st]) => (
                <div key={a} className={`pipe-step tone-${tone}`}>
                  <span className="ic"><Icon aria-hidden="true" /></span>
                  <span className="grow"><b style={{ fontWeight: 500 }}>{a}</b><span className="tiny dim" style={{ display: "block" }}>{b}</span></span>
                  <span className={`badge badge-${tone}`}>{st}</span>
                </div>
              ))}
            </div>
          </div>
          <div>
            <SectionHead eyebrow="4 · Briefs" title="One portal for every brief and delivery." lead="Once your contract is signed you get a private link to the freelancer portal. Read your briefs, hand in your work, and see whether it was accepted or sent back with notes." />
            <a className="arrow-link" href={BRIEFS_URL}>Open the freelancer portal <ArrowUpRight aria-hidden="true" /></a>
          </div>
        </div>
      </section>

      <section className="section" id="pay">
        <div className="wrap">
          <SectionHead eyebrow="5 · Pay" title="Weekly, in NEAR, to an address only you control." />
          <div className="grid-4">
            {[
              [BadgeCheck, "Prove your address", "Before your first payout you prove you own your NEAR address. Nobody can redirect your pay."],
              [FileText, "Tax form on file", "A W-9 or W-8BEN, signed once."],
              [Wallet, "Paid every week", "Accepted work × your rate. Under $50 goes out on its own; $50+ waits for the board's quick sign-off."],
              [null, "Lands in seconds", "NEAR transfers settle fast and cost a fraction of a cent. Every payment is verifiable on chain."],
            ].map(([Icon, t, d], i) => (
              <div key={t} className="panel pad feature spot" data-reveal style={{ "--d": `${i * 0.06}s` }}>
                <span className="card-icon">{Icon ? <Icon aria-hidden="true" /> : <span style={{ fill: "currentColor" }}><BrandIcon icon={siNear} size={20} /></span>}</span>
                <h3>{t}</h3>
                <p>{d}</p>
              </div>
            ))}
          </div>
          <p className="tiny dim mt-m">Hired through Upwork instead? Then you're paid through Upwork, under its terms, not in NEAR.</p>
        </div>
      </section>

      <section className="section" id="your-data">
        <div className="wrap" style={{ maxWidth: 920 }}>
          <SectionHead center eyebrow="6 · Your data and your questions" title="Clear rules, kept." />
          <Faq items={FAQ} />
          <p className="center mt-m"><a className="link" href="/privacy/">Read how Combox handles data</a></p>
        </div>
      </section>
    </>
  );
}
