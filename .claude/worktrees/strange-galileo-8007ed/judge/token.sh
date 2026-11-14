#!/usr/bin/env bash
# Judge mode: print an MFA-verified (aal2) dashboard session token for the
# judge admin, signed with the local judge secret from ./judge/run.sh.
# Stands in for a Supabase session; valid for 12 hours.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/judge/out/judge-admin.env"
python3 - "$JUDGE_JWT_SECRET" "$AXOM_JUDGE_ADMIN_ID" <<'PY'
import base64, hashlib, hmac, json, sys, time
secret, sub = sys.argv[1].encode(), sys.argv[2]
b64 = lambda d: base64.urlsafe_b64encode(json.dumps(d, separators=(",", ":")).encode()).rstrip(b"=")
head, body = b64({"alg": "HS256", "typ": "JWT"}), b64({"sub": sub, "email": "judge@judge.example", "aal": "aal2", "session_id": "judge", "aud": "authenticated", "exp": int(time.time()) + 43200})
sig = base64.urlsafe_b64encode(hmac.new(secret, head + b"." + body, hashlib.sha256).digest()).rstrip(b"=")
print((head + b"." + body + b"." + sig).decode())
PY
