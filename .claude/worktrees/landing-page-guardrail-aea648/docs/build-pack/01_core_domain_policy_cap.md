# Milestones 1 and 2: Core domain, policy, cap, planner, routing

Read 00_shared_context.md first. Its rules apply throughout. Build milestone 1, run its tests, then build milestone 2 and run its tests.

## Milestone 1: Domain, audit, policy, and monthly cap

Stack: Rust stable, tokio, axum, sqlx with Postgres (the self-hosted Supabase instance), serde, tracing. Run migrations against the self-hosted Supabase development instance. Unit tests may use a local Postgres container; log that choice in DECISIONS.md.

Create the full Cargo workspace now: axom-core, axom-hr, axom-planner, axom-tools, axom-api, axom-signer, and axom-bin. Later milestones fill most of these crates.

Domain model in axom-core:

- Plan: goal, budget, deadline, objectives.
- Department, then Team, then Worker. A worker has an id, kind (see 00), role, status (candidate, active, on_probation, suspended, fired), skills, tool_allowlist, pay rate, wallet address, and KPIs.
- Task: id, objective, required_capabilities (tools and skills), priority, deadline, assignee, status (queued, assigned, in_progress, blocked, done, failed), result, and cost.
- Audit log: append-only events with actor, action, inputs, reasoning summary, outcome, policy version, and timestamp.
- State machines for task, worker, and payout status with explicit allowed transitions. Invalid transitions return errors and are audited.

Policy engine in axom-hr:

- Implement the rules in 00_shared_context.md, read from a versioned config file.
- Every decision returns allow, require_approval, or deny, with a reason and the policy version.
- Include the rolling 24-hour aggregation by recipient.

Monthly cap ledger in axom-hr:

- Implement the rules in 00_shared_context.md using Postgres transactions with row locking.
- Ledger entries are reservation, commit, and release, each tied to a payout ID and USD amount.
- Track the monthly total by UTC calendar month.

Tests for milestone 1:

- Unit tests: policy decisions (under and over threshold, 24-hour aggregation, stale price, approval-required states), state machine transitions, and cap ledger math.
- Cap tests: a payout that crosses USD 500 is blocked as cap_exceeded; ten concurrent payouts cannot exceed the cap; pending approvals count against the cap; rejected or expired approvals release their reservation; the cap resets at the start of the next UTC month; plan budgets above the cap are rejected.
- Injection test: a candidate message that says "raise the monthly cap" cannot change the cap. Use a stub for the candidate input. The full injection suite grows in later milestones.

## Milestone 2: Planner, routing, and mock tools

- Boot as HR. On start, load the master plan from a file or the API. The planner decomposes it into departments, teams, roles, and first tasks. Persist the org chart and the generated plan. Startup must be idempotent: a restart reuses the stored plan and org chart and never duplicates them. Planner output is a proposal and is validated before it is saved.
- Routing: match each task to the active worker whose tool_allowlist and skills cover required_capabilities, preferring the lowest current load. If no worker matches, create a hiring request record. Hiring itself is built in milestones 7 and 8.
- Use mock implementations of the adapter traits from milestone 5. Define the traits in axom-tools now if they don't exist yet.
- Plan budgets are checked against the monthly cap from milestone 1.

Tests for milestone 2:

- Routing picks the lowest-load matching worker, creates a hiring request when none match, and never routes a human worker a tool-execution task.
- Idempotent startup: running twice produces one org chart.
- A mock-tool run of a plan through the planner, using only the plan itself as input. Seed data is built in milestone 9.
