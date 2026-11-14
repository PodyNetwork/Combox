#!/usr/bin/env bash
# One-time OpenBao setup, run by the OWNER with their admin identity
# (never by ForeMan). Creates the treasury key, the signer policy, and a token
# for axom-signer. The audit log is declared in the server config
# (openbao.hcl): OpenBao 2.x refuses to enable audit devices through the API.
#
#   export BAO_ADDR=https://openbao.example.com
#   export BAO_TOKEN=<owner admin token>
#   ./setup-treasury.sh
#
# The treasury key is ed25519, not exportable, with no plaintext backup, and
# cannot be deleted. Its private key never leaves OpenBao. The NEAR treasury
# address is the implicit account: lowercase hex of the 32-byte public key.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

# Every request, including every sign request, must be audited.
# (Config-declared audit devices can take a moment to register after start.)
for _ in 1 2 3 4 5 6 7 8 9 10; do
  bao audit list 2>/dev/null | grep -q '^file/' && break
  sleep 1
done
if ! bao audit list 2>/dev/null | grep -q '^file/'; then
  # A refused or unreachable request looks the same as "no audit device"
  # above; tell them apart so the message points at the real problem.
  out="$(bao audit list 2>&1 || true)"
  if echo "$out" | grep -qiE 'permission denied|connection refused|dial tcp|no such host|code: 40|x509|tls'; then
    echo "Can't check OpenBao's audit devices at ${BAO_ADDR:-?}:" >&2
    echo "$out" | head -5 >&2
    echo "Check that BAO_ADDR is the right server and BAO_TOKEN is an admin token for it." >&2
  else
    echo "No file audit device. Add the audit stanza from openbao.hcl to the server config and restart." >&2
  fi
  exit 1
fi

bao secrets list | grep -q '^transit/' || bao secrets enable transit

if ! bao read -format=json transit/keys/axom-treasury >/dev/null 2>&1; then
  bao write transit/keys/axom-treasury type=ed25519 exportable=false allow_plaintext_backup=false
fi
# deletion_allowed defaults to false; set it explicitly so it is visible.
bao write transit/keys/axom-treasury/config deletion_allowed=false exportable=false

# Through stdin, so this also works with bao running in a container
# (./scripts/setup).
bao policy write axom-signer - < "$HERE/axom-signer.hcl"

KEY_JSON="$(bao read -format=json transit/keys/axom-treasury)"
if command -v jq >/dev/null; then
  PUB_B64="$(echo "$KEY_JSON" | jq -r '.data.keys[(.data.latest_version|tostring)].public_key')"
elif command -v python3 >/dev/null; then
  PUB_B64="$(echo "$KEY_JSON" | python3 -c 'import json,sys; d=json.load(sys.stdin)["data"]; print(d["keys"][str(d["latest_version"])]["public_key"])')"
else
  # A new key has one version, so its only public_key is the latest.
  [[ "$(echo "$KEY_JSON" | tr -d ' \n' | sed -n 's/.*"latest_version":\([0-9]*\).*/\1/p')" == 1 ]] \
    || { echo "Install jq or python3 to read a rotated treasury key." >&2; exit 1; }
  PUB_B64="$(echo "$KEY_JSON" | tr -d ' \n' | sed -n 's/.*"public_key":"\([^"]*\)".*/\1/p')"
fi
echo "Treasury implicit account (fund this from your cold wallet):"
echo "$PUB_B64" | base64 -d | od -An -tx1 | tr -d ' \n'; echo

echo "Token for axom-signer (set it as the signer's OPENBAO_TOKEN environment variable; never save it to a file):"
bao token create -policy=axom-signer -no-default-policy -orphan -period=768h -display-name=axom-signer -field=token
