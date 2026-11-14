# Milestone 6: LiveKit meeting platform

Read 00_shared_context.md first. Its rules apply throughout this milestone.

## Infrastructure

- Self-host LiveKit on Coolify. Do not use LiveKit Cloud.
- Run a self-hosted LiveKit server on Coolify with TURN enabled. Open the UDP media port range and the TLS ports, and point a domain at the server. Keep the API key and secret in environment variables. Run one development instance and one production instance.
- Issue participant tokens from the admin backend with the LiveKit server SDK, using short expiry.
- Implement the LiveKit adapter trait from milestone 5 against this server.

## Meeting agent (/meet-agent, Python, LiveKit Agents SDK)

Run the agent as a Coolify service beside the LiveKit server. Speech-to-text, text-to-speech, and the LLM are external provider APIs configured through environment variables. Log the chosen providers in DECISIONS.md.

- Joins a room with instructions and questions.
- Opens with the AI disclosure and asks for recording consent. Proceeds only with consent, and stores the consent record with the meeting.
- Speaks through TTS, listens through STT, asks the prepared questions, and asks follow-ups.
- Records the transcript and writes a summary.
- Posts the summary to an Combox callback.
- Must not make offers, promise pay, or commit to terms. Those happen only through the policy flow in milestones 7 and 8.
- Scores candidates on job-related criteria only, using a rubric. Store the rubric and score with the meeting.

## Participant UI (/meet-web, React, livekit-components-react)

- Join by link.
- Camera and mic controls.
- Room chat.

## Summaries

- Send through our SMTP server to the meeting's invitees and the owner only, unless the owner adds a recipient in the dashboard.
- Save each summary as a CRM note through the CRM adapter.

## Tests

- Meeting-flow test with a fake participant: disclosure and consent are recorded before any questions are asked.
- If consent is declined, the agent ends the interview without asking questions.
- When a participant asks about pay or terms, the agent refuses to commit.
- send_summary cannot reach any recipient outside the invitees and the owner, unless the owner added them.
- Injection test: a transcript that instructs the agent to send data to an outside address is blocked.
