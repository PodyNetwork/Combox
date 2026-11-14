# Rust ForeMan: Build Prompts

Run these files in order with an AI coding agent. Start every session with 00_shared_context.md, then add the one milestone file for that session.

| File | Milestone | Purpose |
|---|---|---|
| 00_shared_context.md | all | Stack, hosted services, policy engine, monthly cap, and rules. Attach to every session. |
| 01_core_domain_policy_cap.md | 1 and 2 | Domain model, audit log, policy engine, monthly cap ledger, planner, routing |
| 02_performance_firing.md | 3 | Reviews, firing, owner notifications |
| 03_payroll_signer.md | 4 | OpenBao wallet service, signer, payouts, approvals, tax and address gates |
| 04_tool_adapters.md | 5 | Adapters for DocuSeal, Warmbly, Postiz, CRM, LiveKit, SMTP, task intake, Upwork |
| 05_livekit_platform.md | 6 | Self-hosted LiveKit, meeting agent, participant UI, summaries |
| 06_admin_dashboard_mcp.md | 7 | Minimal admin dashboard and MCP server |
| 07_upwork_freelancers.md | 8 | Upwork, freelancer hiring, and contracts |
| 08_end_to_end_demo.md | 9 | Seed data, documentation, acceptance |
| 09_hackathon_submission.md | 10 | NEAR Hackathon packaging, judge checks, and submission |
| NEAR_HACKATHON.md | reference | Organizer rules, tracks, and the submission contract, copied from the live page |

Rules for running:

- Each session builds one milestone and ends with its tests passing.
- DECISIONS.md carries state between sessions. Commit it at the end of each milestone.
- Live mode stays off until the legal review listed in 00 is complete.
- Hackathon submission happens only in milestone 10, and only after the human confirms the payload.
