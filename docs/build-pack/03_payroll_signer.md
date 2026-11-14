# Milestone 4: Wallet service, signer, payouts, approvals, and tax and address gates

Read 00_shared_context.md first. Its rules apply throughout this milestone.

## Wallet as a service (OpenBao)

- Default to NEAR testnet. Use mainnet only when NEAR_NETWORK=mainnet is set explicitly.
- OpenBao holds one hot treasury key. Create it with the transit engine as an ed25519 key that is not exportable and cannot be deleted. The private key never leaves OpenBao.
- The treasury address is the NEAR implicit account for the key's public key: the lowercase hex encoding of the 32-byte public key. The owner creates the key once. No agent creates keys.
- Keep only the working balance in the treasury, at most one month's budget under the cap. The owner tops it up from a cold wallet that ForeMan does not control.
- OpenBao access policies: axom-signer may call sign on the treasury key and read its public key, and nothing else. Only the owner's admin identity can create, rotate, or disable keys. ForeMan never uses that identity.
- Enable OpenBao's audit log, so every sign request is recorded.
- After a VPS reboot, OpenBao stays sealed until the owner unseals it, and payouts pause in the meantime. That is the intended failure mode.
- Before each sign request, axom-signer checks the recipient against verified addresses and the amount against the authorization, and rejects any mismatch.
- In this milestone, verify three things before any real payout: transit signing produces valid NEAR transactions (NEAR signs a SHA-256 hash of the transaction, so check that signing that hash yields a signature NEAR accepts), the public key converts to the expected implicit account, and a sealed OpenBao makes payouts fail safely. If any check fails, stop and log the blocker in DECISIONS.md. Never store keys in the repo or in environment variables.

## Signer

- axom-signer is a separate service and the only code that calls OpenBao for signing. axom-core and axom-hr cannot call OpenBao directly.
- axom-signer checks the policy authorization itself before every call, because OpenBao's controls do not replace ours.
- The signer accepts a payout only with a valid authorization from the policy engine: either the automatic under-threshold attestation, or a human approval signature from the dashboard.

## Payout flow

- Each pay period, compute what each worker is owed from completed work and their pay rate. Create a payout record for each.
- Each payout record stores recipient, USD and NEAR amounts, rate used, policy version, cap reservation ID, and status.
- Statuses: pending_approval, approved, sent, finalized, failed, rejected, expired, cap_exceeded.
- Convert USD to NEAR using the price feed with the 15-minute staleness limit. A stale price forces approval.
- Reserve the cap amount when the payout is created, using the ledger from milestone 1.
- Payouts under USD 50 send an automatic attestation to the signer.
- Payouts of USD 50 or more stay in pending_approval until approved in the dashboard.
- Retry failed transactions with exponential backoff. After the final retry, raise an alert.

## Approval API

- Build the approval endpoints in axom-api. They accept approve or reject only from a named admin with an MFA-verified session. An approval produces a signed record that axom-signer verifies.
- No approval endpoint is exposed through MCP. The approval UI is built in milestone 7.
- Approving a payout never overrides the cap.

## Gates before a worker's first payout

- Verify the NEAR address with a challenge signature.
- Confirm a tax form (W-9 or W-8BEN, collected via DocuSeal) is on file.
- Block the payout if either check is missing. Use a clear status and reason.

## Tests

- Signer test on NEAR testnet: a payout under the threshold is sent and finalized.
- A payout of USD 50 or more stays in pending_approval until approved.
- Rejected and expired approvals release the cap reservation.
- A stale price forces approval.
- A missing tax form or unverified address blocks the first payout.
- The signer rejects a payout with no valid authorization.
- A sign request whose recipient is not in the verified addresses is rejected, and so is one whose amount differs from the authorization.
- With OpenBao sealed, payouts fail safely and nothing is marked sent.
- An approval from a session without MFA is rejected.
- Retries back off, and an alert is raised after the final attempt.
- Payroll math: the amount owed equals completed work times the pay rate for the period.
