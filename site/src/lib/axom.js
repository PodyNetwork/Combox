// Small, faithful copies of Combox's own rules, so the demos on this site
// behave the way the product does:
//   - evaluatePayout mirrors the payroll gates, axom-hr's policy engine
//     (crates/axom-hr/src/policy.rs) and the monthly cap ledger.
//   - plan mirrors the rule-based planner (config/planner.toml).
//   - firingEligible mirrors config/performance.toml.
import { FIRING, POLICY } from "./config.js";

const money = (n) => Number(n).toFixed(2);

/**
 * Walk one payout through the checks it meets on its way out.
 * Returns { outcome: "sent" | "held" | "blocked", steps, reason }.
 */
export function evaluatePayout({ amount, window24h, monthUsed, address, taxForm, priceFresh, killSwitch, fee = 0.01, threshold = POLICY.thresholdUsd, cap = POLICY.capUsd }) {
  const steps = [];
  let stop = null;
  let hold = null;
  const push = (key, label, ok, detail, kind = "fail") => {
    if (stop) return steps.push({ key, label, status: "skip", detail: "Not reached" });
    if (ok) return steps.push({ key, label, status: "pass", detail: detail.pass });
    steps.push({ key, label, status: kind, detail: detail.fail });
    if (kind === "fail") stop = detail.fail;
    else hold = hold || detail.fail;
  };

  push("kill", "Kill switch", !killSwitch, {
    pass: "Off. Automation is running.",
    fail: "Kill switch is on: all tool calls, payouts, and outbound actions are halted.",
  });
  push("address", "Verified NEAR address", address, {
    pass: "The worker proved they own the receiving address.",
    fail: "No verified NEAR address on file. Blocked by the first-payout gate.",
  });
  push("tax", "Tax form on file", taxForm, {
    pass: "W-9 / W-8BEN received.",
    fail: "No tax form (W-9 or W-8BEN) on file. Blocked by the first-payout gate.",
  });
  push(
    "price",
    "Fresh NEAR price",
    priceFresh,
    {
      pass: `Price quote is under ${POLICY.priceMaxAgeSecs / 60} minutes old.`,
      fail: `NEAR price is stale (limit ${POLICY.priceMaxAgeSecs}s). Approval required.`,
    },
    "hold",
  );
  const aggregate = window24h + amount;
  push(
    "threshold",
    `$${threshold} approval threshold`,
    aggregate < threshold,
    {
      pass: `Recipient total ${money(aggregate)} USD over ${POLICY.windowHours}h is under the ${threshold} USD threshold.`,
      fail: `Recipient total ${money(aggregate)} USD over ${POLICY.windowHours}h (this payout ${money(amount)}) is at or above the ${threshold} USD threshold.`,
    },
    "hold",
  );
  const requested = amount + fee;
  push("cap", `$${cap} monthly cap`, monthUsed + requested <= cap, {
    pass: `${money(monthUsed)} used + ${money(requested)} requested fits the ${cap} USD cap.`,
    fail: `Monthly cap exceeded: ${money(monthUsed)} used + ${money(requested)} requested > ${cap} cap. Approval cannot override this.`,
  });

  if (stop) return { outcome: "blocked", steps, reason: stop };
  if (hold) return { outcome: "held", steps, reason: hold };
  return { outcome: "sent", steps, reason: "Signed authorization issued. The signer re-checks it and asks OpenBao to sign." };
}

// ------------------------------------------------------------------ planner

export const DEPARTMENTS = [
  {
    name: "Engineering",
    team: "Product Engineering",
    keywords: ["build", "develop", "mvp", "api", "code", "product", "deploy", "infrastructure", "app", "feature", "backend", "frontend"],
    roles: [{ title: "Software Engineer Agent", kind: "ai", headcount: 2, rate: 2, tools: ["code_repo", "web_research"] }],
  },
  {
    name: "Marketing",
    team: "Growth",
    keywords: ["marketing", "launch", "social", "content", "brand", "audience", "campaign", "blog", "seo", "announce"],
    roles: [{ title: "Content Marketer Agent", kind: "ai", headcount: 1, rate: 1.5, tools: ["postiz", "web_research"] }],
  },
  {
    name: "Sales",
    team: "Outbound",
    keywords: ["sales", "customers", "leads", "outreach", "pipeline", "revenue", "pricing", "deals"],
    roles: [{ title: "Sales Development Agent", kind: "ai", headcount: 1, rate: 1.5, tools: ["warmbly", "crm"] }],
  },
  {
    name: "Design",
    team: "Brand Design",
    keywords: ["design", "logo", "ui", "ux", "visual", "mockup", "illustration"],
    roles: [{ title: "Brand Designer", kind: "human", headcount: 1, rate: 25, tools: [] }],
  },
  {
    name: "Operations",
    team: "Back Office",
    keywords: ["legal", "contract", "hire", "hiring", "support", "operations", "finance", "compliance"],
    roles: [{ title: "Operations Agent", kind: "ai", headcount: 1, rate: 1, tools: ["docuseal", "smtp"] }],
  },
];
const FALLBACK = "Operations";

const tokens = (s) => new Set(s.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean));

/** The department an objective lands in: most keyword hits, first on ties. */
export function classify(text) {
  const words = tokens(text);
  let best = null;
  for (const d of DEPARTMENTS) {
    const hits = d.keywords.filter((k) => words.has(k)).length;
    if (hits > 0 && (!best || hits > best.hits)) best = { name: d.name, hits, matched: d.keywords.filter((k) => words.has(k)) };
  }
  return best || { name: FALLBACK, hits: 0, matched: [] };
}

/** Objectives in, org chart and routed tasks out. */
export function plan(objectives) {
  const used = new Map();
  const tasks = objectives
    .map((o) => o.trim())
    .filter(Boolean)
    .map((objective) => {
      const c = classify(objective);
      if (!used.has(c.name)) used.set(c.name, DEPARTMENTS.find((d) => d.name === c.name));
      return { objective, department: c.name, matched: c.matched };
    });

  const departments = DEPARTMENTS.filter((d) => used.has(d.name)).map((d) => {
    const workers = [];
    for (const r of d.roles) {
      for (let i = 0; i < r.headcount; i++) {
        workers.push({
          id: `${d.name}-${r.title}-${i}`,
          title: r.headcount > 1 ? `${r.title} ${i + 1}` : r.title,
          kind: r.kind,
          rate: r.rate,
          tools: r.tools,
          load: 0,
        });
      }
    }
    return { name: d.name, team: d.team, workers };
  });

  // Route: the AI worker in the department with the lightest load; a
  // department with only human roles opens a hiring request instead.
  for (const t of tasks) {
    const dept = departments.find((d) => d.name === t.department);
    const ai = dept.workers.filter((w) => w.kind === "ai").sort((a, b) => a.load - b.load)[0];
    if (ai) {
      ai.load += 1;
      t.assignee = ai.title;
      t.kind = "ai";
    } else {
      t.assignee = "Hiring request → Combox Jobs";
      t.kind = "human";
    }
  }
  return { departments, tasks };
}

// ------------------------------------------------------------------ firing

/** Whether a worker with this history may be fired, and why (not). */
export function firingEligible({ completed, days, badPeriods }) {
  const checks = [
    { label: `At least ${FIRING.minCompletedTasks} completed tasks`, ok: completed >= FIRING.minCompletedTasks, value: `${completed} completed` },
    { label: `At least ${FIRING.minDataDays} days of history`, ok: days >= FIRING.minDataDays, value: `${days} days` },
    { label: `${FIRING.consecutiveBelowPeriods} below-threshold review periods in a row`, ok: badPeriods >= FIRING.consecutiveBelowPeriods, value: `${badPeriods} in a row` },
  ];
  return { checks, fire: checks.every((c) => c.ok) };
}
