import { useMemo, useState } from "react";
import { Bot, Sparkles, UserRound, Wand2 } from "lucide-react";
import { plan } from "../lib/axom.js";

const EXAMPLES = [
  {
    goal: "Launch a SaaS product in 90 days",
    objectives: [
      "Build the MVP web app",
      "Deploy production infrastructure",
      "Design the brand and logo",
      "Run the launch marketing campaign",
      "Start outbound sales",
      "Set up contracts and compliance",
    ],
  },
  {
    goal: "Open an online store for handmade candles",
    objectives: ["Build the store app with checkout", "Design product photos and the logo", "Grow the audience on social", "Handle customer support and returns"],
  },
  {
    goal: "Grow a newsletter to 10,000 readers",
    objectives: ["Write a weekly content calendar and blog posts", "Find sponsors and run outreach to brands", "Design a visual identity for the newsletter"],
  },
];

export default function OrgBuilder() {
  const [goal, setGoal] = useState(EXAMPLES[0].goal);
  const [text, setText] = useState(EXAMPLES[0].objectives.join("\n"));
  const [built, setBuilt] = useState({ goal: EXAMPLES[0].goal, text: EXAMPLES[0].objectives.join("\n"), n: 0 });

  const result = useMemo(() => plan(built.text.split("\n")), [built]);
  const ai = result.departments.reduce((n, d) => n + d.workers.filter((w) => w.kind === "ai").length, 0);
  const hiring = result.tasks.filter((t) => t.kind === "human").length;

  const pick = (ex) => {
    setGoal(ex.goal);
    setText(ex.objectives.join("\n"));
    setBuilt((b) => ({ goal: ex.goal, text: ex.objectives.join("\n"), n: b.n + 1 }));
  };

  return (
    <div className="panel demo spot" data-reveal="zoom">
      <form
        className="demo-controls org-input"
        onSubmit={(e) => {
          e.preventDefault();
          setBuilt((b) => ({ goal, text, n: b.n + 1 }));
        }}
      >
        <span className="demo-tag"><Wand2 aria-hidden="true" /> Write a master plan</span>
        <div className="presets" role="group" aria-label="Example plans">
          {EXAMPLES.map((ex) => (
            <button key={ex.goal} type="button" className="chip" aria-pressed={built.goal === ex.goal && built.text === ex.objectives.join("\n")} onClick={() => pick(ex)}>
              {ex.goal.split(" ").slice(0, 4).join(" ")}…
            </button>
          ))}
        </div>
        <label className="small muted" htmlFor="goal">Goal</label>
        <input id="goal" value={goal} maxLength={90} onChange={(e) => setGoal(e.target.value)} style={{ marginTop: -14 }} />
        <label className="small muted" htmlFor="objectives">Objectives, one per line</label>
        <textarea id="objectives" rows={7} value={text} maxLength={900} onChange={(e) => setText(e.target.value)} style={{ marginTop: -14 }} />
        <button className="btn btn-lime" type="submit">
          <Sparkles aria-hidden="true" /> Build the org chart
        </button>
        <p className="tiny dim">
          Runs the same keyword rules as ForeMan's rule-based planner (<span className="mono">config/planner.toml</span>), right here in your browser.
        </p>
      </form>

      <div className="demo-out" key={built.n} aria-live="polite">
        <div className="row" style={{ justifyContent: "space-between" }}>
          <span className="demo-tag">Proposed org chart</span>
          <div className="row" style={{ gap: 6 }}>
            <span className="badge badge-violet">{ai} AI workers</span>
            <span className="badge badge-sky">{hiring} hiring {hiring === 1 ? "request" : "requests"}</span>
          </div>
        </div>
        {result.tasks.length === 0 ? (
          <p className="muted small">Add at least one objective.</p>
        ) : (
          <>
            <div className="org-chart">
              <div className="org-root">{built.goal || "Your goal"}</div>
              <div className="org-trunk" />
              <div className="org-depts">
                {result.departments.map((d, i) => (
                  <div className="org-dept" key={d.name} style={{ "--i": i }}>
                    <h5>{d.name}</h5>
                    <div className="team">{d.team}</div>
                    {d.workers.map((w, j) => (
                      <div key={w.id} className={`org-worker ${w.kind}`} style={{ "--i": i, "--j": j }} title={w.kind === "ai" ? `Tools: ${w.tools.join(", ")}` : "Hired through ForeMan Jobs"}>
                        {w.kind === "ai" ? <Bot aria-hidden="true" /> : <UserRound aria-hidden="true" />}
                        <span>{w.title}</span>
                      </div>
                    ))}
                  </div>
                ))}
              </div>
            </div>
            <div>
              <span className="demo-tag">First tasks, routed</span>
              <ul className="task-list mt-s">
                {result.tasks.map((t, i) => (
                  <li key={i} style={{ "--i": i }}>
                    <span className="obj">
                      {t.objective}
                      <small>{t.department}{t.matched.length ? ` · matched “${t.matched.join("”, “")}”` : " · no keyword, fallback department"}</small>
                    </span>
                    <span className={`badge ${t.kind === "ai" ? "badge-violet" : "badge-sky"}`}>{t.assignee}</span>
                  </li>
                ))}
              </ul>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
