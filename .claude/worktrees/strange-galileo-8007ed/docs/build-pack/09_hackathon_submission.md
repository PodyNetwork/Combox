# Milestone 10: NEAR Hackathon packaging and submission

Read 00_shared_context.md and NEAR_HACKATHON.md first. Run this milestone after milestone 9 passes. The organizers' rules in NEAR_HACKATHON.md override any convenience in the milestones above. The human re-checks the live rules at https://nearhackathon.com/HACKATHON.md before submitting. If the live rules differ from the snapshot, log the difference in DECISIONS.md and follow the live rules.

## Dates (UTC)

- Judged code window: from 2026-10-19 (Q-001) to 2026-11-14T23:59:00Z. Only the NEAR integration built in this window is judged (Q-002). Commits before 2026-10-19 are baseline.
- Submissions are accepted from 2026-11-01T00:00:00Z to 2026-11-14T23:59:00Z. Outside that window the API answers 403.
- Repository public by: 2026-11-14T23:59:00Z.
- Judging starts 2026-11-16. Results are announced by 2026-11-18.

## Scope and schedule

The human chose the full build: all nine milestones, then this packaging milestone. The window closes in six days, so use these checkpoints (UTC). The human may change the pace.

- 2026-11-09: milestones 1 and 2 pass their tests.
- 2026-11-10: milestones 3 and 4 pass, including the OpenBao and NEAR testnet payout test.
- 2026-11-11: milestones 5 and 6 pass.
- 2026-11-12: milestones 7 and 8 pass.
- 2026-11-13: milestone 9 passes, judge mode runs from a clean clone, and the demo post is drafted.
- 2026-11-13 evening: this milestone runs. The human publishes the demo post, makes the repository public, and confirms the payload.
- 2026-11-14: buffer only. Submit before 23:59 UTC if anything is still pending.

If a checkpoint slips, log the gap in DECISIONS.md and tell the human the same day, with the options for recovering the time. The human decides what, if anything, to cut. Never drop a milestone silently. Judge mode and the NEAR testnet payout are on the judging path, so build them in order rather than leaving them for last.

## Repository steps

- Tag the baseline `pre-hackathon` at the last commit before 2026-10-19, if the repository already has code. Tag the submitted state `hackathon-submission` (Q-003).
- Add a README section, "Built at NEAR HACKATHON", that links the diff between the two tags and lists pre-existing libraries and code (Q-005).
- Add a LICENSE file. MIT is fine (Q-014).
- Disclose in the README any pre-existing code and any third-party services used (Q-005). State that the NEAR integration is the payout path, signed through OpenBao.
- Run a secret scan, such as gitleaks, over the full history before the repository goes public. Nothing from .env files, OpenBao tokens, LLM keys, or SMTP credentials may appear in any commit.

## Judge reproducibility

Judges must be able to run the project without our VPS (Q-012). Confirm the following before the repository goes public:

- Judge mode (milestone 9) runs from a clean clone on a machine that has no access to the VPS.
- The README gives "do X, see Y" steps a judge can follow, from clone to a visible result, in under 10 minutes.
- The README includes at least one NEAR testnet transaction hash from a judge-mode payout, so judges can verify the NEAR integration on an explorer.
- A hosted demo is a plus. If it runs, keep it up through 2026-11-25 (Q-012).

## Demo post

The human writes and publishes the post on X. You may draft the text and suggest the demo clip. Never post it yourself.

- The post says the project was built at NEAR HACKATHON, tags @NEARProtocol and #NEARHackathon, and attaches the demo video or screenshots. @NearLegion and @NearBuilders are optional.
- Lead with the user problem: a company run by AI agents that still needs a human to approve money.
- Show one complete moment: an agent hires a freelancer, the freelancer finishes the work, and a payout under the cap goes out on NEAR testnet while a larger one waits for approval.
- Explain NEAR's role concretely: payouts are signed by a key that never leaves OpenBao, and every payout is checked against a monthly cap and an approval threshold.
- End with the repository link and a specific question for feedback.

## Tracks

The human chose these three tracks: `business`, `agents`, and `security`. Track ids and why each fits:

- `business` (Autonomous Business): AI-run treasury, procurement, and payroll.
- `agents` (Agent Economy): agents that hire other agents and freelancers and pay them.
- `security` (AI Security & Defense): spending limits, approval gates, and a signer that the AI cannot bypass.

Put the chosen ids in the submission's `tracks` array. The rules allow one or more tracks per project, and the project still counts as one submission.

## Human-only inputs

Ask the human for each value. Never guess, generate, or fill in a placeholder:

- Project name (2-80 characters)
- Team lead's name (2-80 characters)
- Contact email (max 160 characters)
- Team size: `1`, `2 - 4`, `4 - 6`, `6 - 8`, or `8+`
- Tracks: `business`, `agents`, and `security`, as confirmed by the human. Ask again before changing them.
- Prize payout NEAR account on mainnet. The human creates it in a NEAR wallet. This is separate from Combox's treasury, and Combox never receives or handles it.
- Public repository link
- Public demo post link
- Idea text: the problem, what was built, and how NEAR fits in (20-2000 characters)
- Consent to the contact notice, read to the human verbatim: "I agree that the organizers may use these details to review our submission and contact us about this hackathon."

## Submission contract

`POST https://nearhackathon.com/api/applications` with `Content-Type: application/json`. Body up to 12000 bytes. Send exactly these fields:

- `requestId`: a UUID, generated once per submission with `uuidgen | tr A-Z a-z`. Reuse the same value on any retry.
- `projectName`, `leadName`: 2-80 characters each.
- `email`: a valid email, max 160 characters. Each email may send at most 2 submissions.
- `size`: exactly one of `1`, `2 - 4`, `4 - 6`, `6 - 8`, `8+`.
- `tracks`: an array of track ids, each listed once.
- `nearAddress`: lowercase, 2-64 characters, matching `^(([a-z\d]+[-_])*[a-z\d]+\.)*([a-z\d]+[-_])*[a-z\d]+$`.
- `projectUrl`: an http(s) URL of the public repository, max 300 characters. Each repository is submitted once.
- `demoUrl`: an http(s) URL of the public demo post, max 300 characters.
- `idea`: 20-2000 characters.
- `consent`: `true`, only after the human has agreed to the contact notice.

Add no other fields. `website` is a spam trap: any non-empty value rejects the submission.

Handle the responses this way:

- 201: saved. Tell the human.
- 400: fix the fields named in the response and resend with the same `requestId`.
- 403: submissions are closed or the request looked cross-site. Tell the human. Do not retry.
- 409: the repository or the email is already at its limit. Ask the human. Never change the link or the email to get around it.
- 413: shorten the idea text.
- 503: wait a minute and resend with the same `requestId`.

## Procedure

1. Run the pre-submission checklist below.
2. Build the payload from the human's answers and print it in full.
3. Wait for an explicit yes. Do not send anything before it.
4. Send the request once. If it fails with 503 or a network error, retry with the same `requestId`.
5. Report the result to the human, and record the submission status and date in DECISIONS.md.

## Never without the human's explicit confirmation

- Sending the submission. Print the payload first and wait for a yes. A project gets one attempt.
- Posting on X or any other network.
- Making the repository public, pushing, or force-pushing.
- Signing transactions, spending tokens, or deploying to mainnet. The prize account is never used for Combox's operations.
- Committing secrets.

## Request changes

If the saved submission has a mistake, do not submit again. The same repository is rejected with 409. The human requests changes at https://nearhackathon.com/request-changes. Help the human collect the new values, check every new value against the contract above, and leave the sending to the human. The organizers may message the human to confirm before they change the payout account or the repository.

## Pre-submission checklist

Verify each item and report the result to the human:

- The repository is public, with a LICENSE file, a README that includes judge steps and the "Built at NEAR HACKATHON" section, and both tags.
- Judge mode runs from a clean clone without the VPS.
- The secret scan passes over the full history.
- The README links a NEAR testnet transaction from a judge-mode payout.
- The demo post is live, and its link is recorded.
- The payload has been printed and approved by the human.
- The submission window is open (it opens 2026-11-01), and the submission is sent before 2026-11-14T23:59:00Z.
