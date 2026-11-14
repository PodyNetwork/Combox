#!/usr/bin/env bash
# Tested backup and restore for both stateful services ForeMan owns.
#   SOURCE_DATABASE_URL=postgres://... ADMIN_DATABASE_URL=postgres://.../postgres BAO=/path/to/bao ./test_backup_restore.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d)"; trap 'kill ${BAO_PID:-0} 2>/dev/null || true; rm -rf "$WORK"' EXIT
: "${SOURCE_DATABASE_URL:?}" "${ADMIN_DATABASE_URL:?}" "${BAO:?}"
export PATH="$(dirname "$BAO"):$PATH"

echo "== Postgres"
DUMP="$(DATABASE_URL="$SOURCE_DATABASE_URL" BACKUP_DIR="$WORK" "$HERE/pg_backup.sh")"
TARGET_DB="axom_restore_test_$$"
psql "$ADMIN_DATABASE_URL" -qc "CREATE DATABASE $TARGET_DB"
TARGET_URL="${ADMIN_DATABASE_URL%/*}/$TARGET_DB"
TARGET_DATABASE_URL="$TARGET_URL" "$HERE/pg_restore.sh" "$DUMP"
for t in audit_log payouts cap_ledger workers tasks meetings; do
  a="$(psql "$SOURCE_DATABASE_URL" -Atc "SELECT count(*) FROM axom.$t")"
  b="$(psql "$TARGET_URL" -Atc "SELECT count(*) FROM axom.$t")"
  [[ "$a" == "$b" ]] || { echo "MISMATCH axom.$t: $a vs $b"; exit 1; }
  echo "  axom.$t: $a rows restored"
done
# The append-only trigger survives the restore.
if psql "$TARGET_URL" -qc "DELETE FROM axom.audit_log" 2>/dev/null; then echo "audit log not protected after restore"; exit 1; fi
echo "  audit_log immutability trigger present"
psql "$ADMIN_DATABASE_URL" -qc "DROP DATABASE $TARGET_DB"

echo "== OpenBao (raft storage, not dev mode)"
start_bao() {
  mkdir -p "$WORK/raft"
  cat > "$WORK/bao.hcl" <<HCL
storage "raft" { path = "$WORK/raft" node_id = "n1" }
listener "tcp" { address = "127.0.0.1:18299" tls_disable = true }
api_addr = "http://127.0.0.1:18299"
cluster_addr = "http://127.0.0.1:18298"
disable_mlock = true
HCL
  "$BAO" server -config="$WORK/bao.hcl" > "$WORK/bao.log" 2>&1 & BAO_PID=$!
  for _ in $(seq 1 50); do curl -s http://127.0.0.1:18299/v1/sys/health >/dev/null && break; sleep 0.2; done
}
export BAO_ADDR=http://127.0.0.1:18299
start_bao
INIT="$("$BAO" operator init -key-shares=1 -key-threshold=1 -format=json)"
KEY1="$(echo "$INIT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["unseal_keys_b64"][0])')"
TOK1="$(echo "$INIT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["root_token"])')"
"$BAO" operator unseal "$KEY1" >/dev/null
for _ in $(seq 1 50); do BAO_TOKEN="$TOK1" "$BAO" secrets list >/dev/null 2>&1 && break; sleep 0.2; done
export BAO_TOKEN="$TOK1"
"$BAO" secrets enable transit >/dev/null
"$BAO" write -f transit/keys/axom-treasury type=ed25519 exportable=false >/dev/null
PUB1="$("$BAO" read -format=json transit/keys/axom-treasury | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["keys"]["1"]["public_key"])')"
"$HERE/openbao_snapshot.sh" save "$WORK/bao.snap"
echo "  snapshot saved; treasury key $PUB1"

# Disaster: lose the server entirely.
kill "$BAO_PID"; wait "$BAO_PID" 2>/dev/null || true; rm -rf "$WORK/raft"
start_bao
INIT2="$("$BAO" operator init -key-shares=1 -key-threshold=1 -format=json)"
KEY2="$(echo "$INIT2" | python3 -c 'import json,sys; print(json.load(sys.stdin)["unseal_keys_b64"][0])')"
export BAO_TOKEN="$(echo "$INIT2" | python3 -c 'import json,sys; print(json.load(sys.stdin)["root_token"])')"
"$BAO" operator unseal "$KEY2" >/dev/null
for _ in $(seq 1 50); do "$BAO" secrets list >/dev/null 2>&1 && break; sleep 0.2; done
"$HERE/openbao_snapshot.sh" restore "$WORK/bao.snap"
# The restored data is sealed with the ORIGINAL keys.
for _ in $(seq 1 50); do "$BAO" status -format=json 2>/dev/null | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin)["sealed"] else 1)' && break; sleep 0.2; done
"$BAO" operator unseal "$KEY1" >/dev/null
export BAO_TOKEN="$TOK1"
for _ in $(seq 1 50); do "$BAO" read transit/keys/axom-treasury >/dev/null 2>&1 && break; sleep 0.2; done
PUB2="$("$BAO" read -format=json transit/keys/axom-treasury | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["keys"]["1"]["public_key"])')"
[[ "$PUB1" == "$PUB2" ]] || { echo "treasury key changed after restore"; exit 1; }
echo "  restored into a fresh server; treasury key unchanged ($PUB2)"
echo "== backup and restore OK"
