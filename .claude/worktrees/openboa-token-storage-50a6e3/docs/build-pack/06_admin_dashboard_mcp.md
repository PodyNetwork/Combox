# Milestone 7: Admin dashboard and MCP server

Read 00_shared_context.md first. Its rules apply throughout this milestone.

## Admin dashboard (/admin, React and Tailwind)

Keep the dashboard minimal. Admins sign in through self-hosted Supabase Auth with TOTP MFA. Build only the screens below, with no charts or extra analytics:

- Meetings: list, status, transcript, summary, and candidate scores.
- Agent dispatch: pick a room and enter instructions and questions.
- Payout approval queue. Approve and reject actions call the approval API from milestone 4.
- Monthly cap meter showing usage, reservations, and pending approvals.
- Cap changes, protected by MFA and audited.
- Firing notification feed.
- Kill switch: one clear action that immediately halts all tool calls and payouts.

## MCP server (on the admin backend, streamable HTTP, bearer-token auth)

Tools:

- create_meeting(title, scheduled_at, invitees)
- dispatch_agent(room_id, instructions, questions[])
- get_transcript(meeting_id)
- get_summary(meeting_id)
- send_summary(meeting_id, recipients[])

Validate every input against a schema, and audit every call.

Hard requirement: do not expose any approval, cap, kill-switch, or policy-changing tool through MCP.

## Tests

- MCP schema validation rejects bad inputs.
- Every MCP call writes an audit event.
- The MCP tool list contains no approval, cap, kill-switch, or policy tools. Write a test that asserts this.
- Payout approval is possible only from an MFA-verified dashboard session.
- The kill switch halts tool calls and payouts within the test's time window.
- The cap meter matches the ledger totals.
