# Milestone 5: Tool adapters

Read 00_shared_context.md first. Its rules apply throughout this milestone.

## Adapters

Build one Rust trait per adapter in axom-tools. Each trait gets a mock implementation for tests.

- DocuSeal (legal, self-hosted on Coolify): create from template, send, poll status, and fetch tax forms. Use only owner-approved templates. Never draft legal language.
- Warmbly (sales, hosted): create and update campaigns, add contacts, and read reply stats.
- Postiz (social, self-hosted on Coolify): schedule posts across channels.
- CRM: OpenDesk if it is confirmed to have an API. Otherwise self-hosted Twenty, and log the choice in DECISIONS.md. Create and update contacts, deals, and notes.
- LiveKit meetings (self-hosted on Coolify): define the trait here. The implementation is built in milestone 6.
- Task intake: an internal queue in axom-api, not an external service. Needs-based submission that accepts a task with requirements and returns its queue position.
- SMTP: define the trait here. Send summaries, owner alerts, and calendar invites (ICS attachments) through our SMTP server.
- Upwork (hosted freelancing MCP): define the trait here. The implementation is built in milestone 8. The trait covers posting jobs, listing applicants, and messaging candidates.

## Rules

- Check each adapter against the vendor's documented API. Log any mismatch in DECISIONS.md.
- Every outbound action goes through the policy engine first. An adapter refuses to act without a policy decision.
- Read credentials from environment variables. Never log them, and redact them from error messages.
- Each adapter has a dry-run mode, which is the default.
- Map vendor errors to typed errors the planner can handle: retryable, permanent, or rate-limited.

## Tests

- Each mock implements its trait fully.
- Each adapter refuses to act without a policy decision.
- Dry-run mode makes no external calls.
- Error mapping covers retryable, permanent, and rate-limited responses.
- Credentials never appear in logs or error text.
