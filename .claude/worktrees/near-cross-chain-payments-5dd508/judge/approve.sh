#!/usr/bin/env bash
# Judge mode: approve the payout waiting in the queue as the judge admin with
# an MFA-verified (aal2) session token minted locally, then watch the signer
# send it on NEAR testnet. (In production the token comes from Supabase Auth
# after TOTP; here run.sh holds the local signing secret.)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/judge/out/judge-admin.env"
API=http://127.0.0.1:8080
TOKEN="$("$ROOT/judge/token.sh")"
PENDING="$(curl -fsS -H "Authorization: Bearer $TOKEN" "$API/payouts/pending")"
ID="$(echo "$PENDING" | python3 -c 'import json,sys; p=json.load(sys.stdin)["payouts"]; print(p[0]["id"] if p else "")')"
[[ -z "$ID" ]] && { echo "nothing waiting for approval"; exit 0; }
echo "approving payout $ID"
curl -fsS -X POST -H "Authorization: Bearer $TOKEN" "$API/payouts/$ID/approve"; echo
echo "the signer sends it within ~10s; watch judge/out/signer.log for the testnet transaction hash"
