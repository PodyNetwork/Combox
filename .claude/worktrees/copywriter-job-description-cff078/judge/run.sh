#!/usr/bin/env bash
# Combox judge mode: one command, no VPS. Starts Postgres (local container),
# OpenBao (dev mode, judge mode only), the signer, the admin API, and runs
# the seeded scenario with the meeting agent's interview logic. External
# services are mocks; NEAR is real testnet.
#
#   ./judge/run.sh            # run the scenario, then keep the API up for approvals
#   ./judge/run.sh --no-serve # run the scenario and stop
#   ./judge/run.sh --stop     # stop anything an earlier run left running
#
# ./scripts/start demo runs this and the web apps together, signed in.
#
# Needs: Rust (cargo), python3, curl, and Docker (or set JUDGE_DATABASE_URL
# to an EMPTY Postgres 14+ database).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/judge/out"
BIN="$ROOT/judge/.bin"
SERVE=1
[[ "${1:-}" == "--no-serve" ]] && SERVE=0
mkdir -p "$OUT" "$BIN"
chmod 700 "$OUT"
# Every process this run starts is listed here, so the next run can stop any
# that outlived a run killed without cleanup (terminal closed, kill -9).
PIDFILE="$OUT/pids"
PIDS=()
PG_CONTAINER=""
cleanup() {
  # Waiting on each one keeps bash from printing a "Terminated" line per process.
  for p in "${PIDS[@]:-}"; do [[ -n "$p" ]] && { kill "$p"; wait "$p"; } 2>/dev/null || true; done
  [[ -n "$PG_CONTAINER" ]] && docker rm -f "$PG_CONTAINER" >/dev/null 2>&1 || true
  rm -f "$PIDFILE"
}
trap cleanup EXIT
say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31m%s\033[0m\n' "$1" >&2; shift; for l in "$@"; do echo "$l" >&2; done; exit 1; }
need() { command -v "$1" >/dev/null || fail "Missing prerequisite: $1" "${2:-}"; }
track() { PIDS+=("$1"); echo "$1" >> "$PIDFILE"; }
need cargo "Install Rust from https://rustup.rs"; need python3; need curl

# Processes of earlier runs: the pid list, plus the judge-only dev OpenBao
# (judge/.bin/bao) of a run that predates the list. Only those whose command
# line points into this checkout count.
judge_pids() {
  { [[ -f "$PIDFILE" ]] && cat "$PIDFILE"; pgrep -f "^$BIN/bao server -dev" 2>/dev/null; } | sort -u || true
}
# A run killed without cleanup (terminal closed, kill -9) leaves orphans that
# hold the ports: stop them. A run that is still going (its run.sh is alive)
# is left alone and reported. "force" (--stop) stops everything.
stop_leftovers() {
  local force="${1:-}" p cmd parent
  for p in $(judge_pids); do
    cmd="$(ps -p "$p" -o command= 2>/dev/null || true)"
    [[ "$cmd" == *"$ROOT"* ]] || continue
    parent="$(ps -p "$(ps -p "$p" -o ppid= | tr -d ' ')" -o command= 2>/dev/null || true)"
    if [[ "$force" != force && "$parent" == *run.sh* ]]; then
      fail "Combox judge mode is already running in another terminal." \
        "Stop it with Ctrl+C there, or run: ./scripts/stop"
    fi
    kill "$p" 2>/dev/null && echo "stopped a leftover from an earlier run: ${cmd:0:80}"
  done
  rm -f "$PIDFILE"
  sleep 0.5
}
# Fail now, naming the process, rather than later with a confusing error.
port_free() {
  command -v lsof >/dev/null || return 0
  local pid
  pid="$(lsof -nP -t -iTCP:"$1" -sTCP:LISTEN 2>/dev/null | head -1 || true)"   # lsof exits 1 when the port is free
  [[ -z "$pid" ]] && return 0
  fail "Port $1 ($2) is already in use." \
    "It's held by: $(ps -p "$pid" -o command= 2>/dev/null | cut -c1-120) (pid $pid)" \
    "Stop that process and run this again."
}
# Docker Desktop on macOS is often installed but not running: start it.
ensure_docker() {
  need docker "Install Docker Desktop from https://www.docker.com/products/docker-desktop/"
  docker info >/dev/null 2>&1 && return 0
  if [[ "$(uname -s)" == Darwin ]] && open -Ra Docker 2>/dev/null; then
    echo "Docker isn't running; starting Docker Desktop..."
    open -a Docker
    for _ in $(seq 1 90); do docker info >/dev/null 2>&1 && return 0; sleep 2; done
  fi
  fail "Docker isn't running." "Start Docker (Docker Desktop, OrbStack, colima, ...) and run this again."
}

remove_judge_postgres() {
  local c
  for c in $(docker ps -aq --filter name=axom-judge-pg- 2>/dev/null); do
    docker rm -f "$c" >/dev/null 2>&1 && echo "removed a leftover judge Postgres container" || true
  done
}

if [[ "${1:-}" == "--stop" ]]; then
  # A live run's run.sh exits (and cleans up) once its API is stopped.
  stop_leftovers force
  if command -v docker >/dev/null && docker info >/dev/null 2>&1; then remove_judge_postgres; fi
  exit 0
fi

stop_leftovers
port_free 18200 OpenBao; port_free 18090 axom-signer
[[ "$SERVE" == 1 ]] && port_free 8080 "Combox API"
rm -f "$OUT/judge-admin.env"   # rewritten once this run's API is about to serve

# --- 1. Postgres -------------------------------------------------------------
say "[1/7] Postgres"
if [[ -n "${JUDGE_DATABASE_URL:-}" ]]; then
  export DATABASE_URL="$JUDGE_DATABASE_URL"
  echo "using JUDGE_DATABASE_URL (must be an empty database)"
else
  ensure_docker
  remove_judge_postgres
  port_free 55432 Postgres
  PG_CONTAINER="axom-judge-pg-$$"
  docker run -d --rm --name "$PG_CONTAINER" -e POSTGRES_USER=axom -e POSTGRES_PASSWORD=axom -e POSTGRES_DB=axom -p 127.0.0.1:55432:5432 postgres:16.4 >/dev/null
  for _ in $(seq 1 60); do docker exec "$PG_CONTAINER" pg_isready -U axom >/dev/null 2>&1 && break; sleep 1; done
  export DATABASE_URL="postgres://axom:axom@localhost:55432/axom"
  echo "local container $PG_CONTAINER on :55432"
fi

# --- 2. Build ----------------------------------------------------------------
say "[2/7] Building Combox (first build takes a few minutes)"
(cd "$ROOT/axom" && cargo build -q -p axom-bin -p axom-signer && cargo build -q -p axom-signer --example testnet_fund)
AXOM="$ROOT/axom/target/debug/axom"
SIGNER="$ROOT/axom/target/debug/axom-signer"
FUND="$ROOT/axom/target/debug/examples/testnet_fund"

# --- 3. OpenBao (dev mode: judge mode only, never on the VPS) ---------------
say "[3/7] OpenBao treasury key"
BAO_VERSION=2.7.1
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) PLAT=linux_amd64 ;; Linux-aarch64|Linux-arm64) PLAT=linux_arm64 ;;
  Darwin-x86_64) PLAT=darwin_amd64 ;; Darwin-arm64) PLAT=darwin_arm64 ;;
  *) echo "unsupported platform $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac
if [[ ! -x "$BIN/bao" ]]; then
  TGZ="openbao_${BAO_VERSION}_${PLAT}.tar.gz"
  curl -fsSL -o "$BIN/$TGZ" "https://github.com/openbao/openbao/releases/download/v${BAO_VERSION}/$TGZ"
  curl -fsSL -o "$BIN/checksums.txt" "https://github.com/openbao/openbao/releases/download/v${BAO_VERSION}/checksums.txt"
  (cd "$BIN" && grep " $TGZ\$" checksums.txt | (sha256sum -c - 2>/dev/null || shasum -a 256 -c -))
  tar -xzf "$BIN/$TGZ" -C "$BIN" bao
fi
BAO_ROOT="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
cat > "$OUT/bao.hcl" <<HCL
audit "file" "file" {
  options {
    file_path = "$OUT/bao-audit.log"
  }
}
HCL
"$BIN/bao" server -dev -dev-root-token-id="$BAO_ROOT" -dev-listen-address=127.0.0.1:18200 -config="$OUT/bao.hcl" > "$OUT/bao.log" 2>&1 &
BAO_PID=$!; track "$BAO_PID"
for _ in $(seq 1 50); do curl -fs http://127.0.0.1:18200/v1/sys/health >/dev/null 2>&1 && break; kill -0 "$BAO_PID" 2>/dev/null || break; sleep 0.2; done
# Check it's this run's OpenBao answering, not one that was already there.
kill -0 "$BAO_PID" 2>/dev/null && curl -fs http://127.0.0.1:18200/v1/sys/health >/dev/null 2>&1 \
  || fail "OpenBao didn't start. Last lines of judge/out/bao.log:" "$(tail -5 "$OUT/bao.log")"
# Keep setup output (it ends with the signer token) in memory, never on disk.
SETUP_OUT="$(PATH="$BIN:$PATH" BAO_ADDR=http://127.0.0.1:18200 BAO_TOKEN="$BAO_ROOT" bash "$ROOT/infra/openbao/setup-treasury.sh")"
TREASURY="$(grep -A1 'implicit account' <<<"$SETUP_OUT" | tail -1)"
SIGNER_TOKEN="$(tail -1 <<<"$SETUP_OUT")"; unset SETUP_OUT
echo "treasury (NEAR testnet implicit account): $TREASURY"

# --- 4. Keys -------------------------------------------------------------------
say "[4/7] Authorization keys"
"$AXOM" keygen > "$OUT/attestation.env"; "$AXOM" keygen > "$OUT/approval.env"; chmod 600 "$OUT"/*.env
val() { grep "^$1=" "$2" | cut -d= -f2-; }
export AXOM_ATTESTATION_SIGNING_KEY="$(val SIGNING_KEY "$OUT/attestation.env")"
export AXOM_ATTESTATION_VERIFY_KEY="$(val VERIFY_KEY "$OUT/attestation.env")"
export AXOM_APPROVAL_SIGNING_KEY="$(val SIGNING_KEY "$OUT/approval.env")"
export AXOM_APPROVAL_VERIFY_KEY="$(val VERIFY_KEY "$OUT/approval.env")"
echo "attestation and approval keys generated (judge/out, not committed)"

# --- 5. Fund the treasury on testnet ------------------------------------------
say "[5/7] Funding the treasury on NEAR testnet"
RPC="${NEAR_RPC_URL:-https://rpc.testnet.near.org}"
balance() { curl -fsS -X POST "$RPC" -H 'content-type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":\"1\",\"method\":\"query\",\"params\":{\"request_type\":\"view_account\",\"finality\":\"final\",\"account_id\":\"$1\"}}" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(int(r.get("result",{}).get("amount","0"))/1e24)'; }
if [[ -n "${JUDGE_COLD_ACCOUNT:-}" && -n "${JUDGE_COLD_KEY_FILE:-}" ]]; then
  COLD="$JUDGE_COLD_ACCOUNT"; COLD_KEY="$JUDGE_COLD_KEY_FILE"
else
  "$AXOM" keygen > "$OUT/cold.env"; chmod 600 "$OUT/cold.env"
  COLD="axom-judge-$(head -c 4 /dev/urandom | od -An -tx1 | tr -d ' \n').testnet"; COLD_KEY="$OUT/cold.env"
  echo "creating throwaway cold wallet $COLD via the testnet helper (faucet)"
  curl -fsS -m 60 -X POST https://helper.testnet.near.org/account -H 'content-type: application/json' \
    -d "{\"newAccountId\":\"$COLD\",\"newAccountPublicKey\":\"$(val VERIFY_KEY "$OUT/cold.env")\"}" >/dev/null \
    || { echo "testnet helper failed. Fund $TREASURY with ~0.3 testnet NEAR (e.g. https://near-faucet.io), then press Enter."; read -r; COLD=""; }
fi
if [[ -n "$COLD" ]]; then
  for _ in $(seq 1 20); do [[ "$(balance "$COLD")" != "0.0" ]] && break; sleep 2; done
  COLD_KEY_FILE="$COLD_KEY" COLD_ACCOUNT="$COLD" TO="$TREASURY" AMOUNT_NEAR=0.3 NEAR_RPC_URL="$RPC" "$FUND"
fi
echo "treasury balance: $(balance "$TREASURY") NEAR"

# --- 6. Signer ------------------------------------------------------------------
say "[6/7] Starting axom-signer (the only process that talks to OpenBao)"
OPENBAO_ADDR=http://127.0.0.1:18200 OPENBAO_TOKEN="$SIGNER_TOKEN" AXOM_SIGNER_BIND=127.0.0.1:18090 \
  AXOM_POLICY_PATH="$ROOT/config/policy.toml" "$SIGNER" > "$OUT/signer.log" 2>&1 &
SIGNER_PID=$!; track "$SIGNER_PID"
sleep 2
kill -0 "$SIGNER_PID" 2>/dev/null || fail "axom-signer stopped right after starting. Last lines of judge/out/signer.log:" "$(tail -5 "$OUT/signer.log")"

# --- 7. Scenario ----------------------------------------------------------------
say "[7/7] Running the seeded scenario"
export AXOM_DEMO_USD_PER_NEAR="${AXOM_DEMO_USD_PER_NEAR:-1000}"
export AXOM_OWNER_EMAIL=owner@judge.example AXOM_ROOT="$ROOT" JUDGE_JWT_SECRET="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
export AXOM_JUDGE_ADMIN_ID="00000000-0000-4000-8000-00000000a11e"
(cd "$ROOT" && "$AXOM" demo)

if [[ "$SERVE" == 1 ]]; then
  say "Combox API on http://127.0.0.1:8080 (judge mode). Approve the held payout with:"
  echo "    ./judge/approve.sh"
  printf 'JUDGE_JWT_SECRET=%s\nAXOM_JUDGE_ADMIN_ID=%s\n' "$JUDGE_JWT_SECRET" "$AXOM_JUDGE_ADMIN_ID" > "$OUT/judge-admin.env"; chmod 600 "$OUT/judge-admin.env"
  echo "Press Ctrl-C to stop everything."
  # In the background so it's on the pid list; Ctrl-C ends the wait and the
  # EXIT trap stops it with everything else.
  (cd "$ROOT" && SUPABASE_JWT_SECRET="$JUDGE_JWT_SECRET" AXOM_DISABLE_PAYROLL=true AXOM_API_BIND=127.0.0.1:8080 exec "$AXOM") &
  API_PID=$!; track "$API_PID"
  wait "$API_PID"
fi
