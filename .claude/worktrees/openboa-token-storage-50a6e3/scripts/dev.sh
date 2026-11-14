#!/usr/bin/env bash
# Run the real ForeMan stack on this machine with the settings in .env.
#
#   cp .env.example .env      # then fill it in (see README, "Running the real stack")
#   ./scripts/dev.sh          # starts everything; Ctrl+C stops everything
#
# Starts: axom (API + scheduler, :8080), axom-signer (needs OpenBao), the
# meeting agent (needs LiveKit), the dashboard (:5174), and the candidate
# site with the jobs board (:5173). A part whose settings are missing is
# skipped with a note instead of failing. Logs go to .dev-logs/.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ ! -f .env ]]; then
  echo "No .env file. Run: cp .env.example .env   then fill it in." >&2
  exit 1
fi
set -a
# shellcheck disable=SC1091
source .env
set +a

need() { [[ -n "${!1:-}" ]]; }
LOGS="$ROOT/.dev-logs"
mkdir -p "$LOGS"
PIDS=()
# Works with macOS's bash 3.2: an empty array is "unbound" there under set -u,
# so expand it with ${PIDS[@]+...}, and there is no `wait -n`.
cleanup() {
  trap - EXIT INT TERM
  echo; echo "Stopping..."
  for pid in ${PIDS[@]+"${PIDS[@]}"}; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

start() { # name, command...
  local name=$1; shift
  # .env (sourced above) holds OpenBao tokens: each process gets only its own.
  local drop
  case "$name" in
    signer) drop=(-u OPENBAO_PROVISIONER_TOKEN) ;;
    axom) drop=(-u OPENBAO_TOKEN) ;;
    *) drop=(-u OPENBAO_TOKEN -u OPENBAO_PROVISIONER_TOKEN) ;;
  esac
  echo "  starting $name (log: .dev-logs/$name.log)"
  env "${drop[@]}" "$@" > "$LOGS/$name.log" 2>&1 &
  PIDS+=($!)
}

for v in DATABASE_URL AXOM_OWNER_EMAIL; do
  need "$v" || { echo "Missing $v in .env (required)." >&2; exit 1; }
done
if ! need SUPABASE_JWKS_URL && ! need SUPABASE_JWT_SECRET; then
  echo "Note: no SUPABASE_JWKS_URL or SUPABASE_JWT_SECRET: dashboard sign-in and approvals are off."
fi
if need AXOM_SECRETS_KEY && ! [[ "$AXOM_SECRETS_KEY" =~ ^[0-9a-fA-F]{64}$ ]]; then
  echo "AXOM_SECRETS_KEY in .env is ${#AXOM_SECRETS_KEY} characters; it must be exactly 64 hex characters." >&2
  echo "  Generate one with: axom/target/debug/axom secrets-key   (keys saved on the Integrations page are then off until it's fixed)" >&2
fi
if [[ "${AXOM_LIVE:-false}" == "true" ]]; then
  echo "AXOM_LIVE=true: outside services will be called for real. Only after the legal review." >&2
fi

echo "Building..."
(cd axom && cargo build -q -p axom-bin -p axom-signer)

echo "Starting ForeMan (network: ${NEAR_NETWORK:-testnet}, live: ${AXOM_LIVE:-false})"
# The API shows the treasury address it gets from the signer, so it's only
# told where the signer is when the signer runs.
if need OPENBAO_ADDR && need OPENBAO_TOKEN && need AXOM_ATTESTATION_VERIFY_KEY && need AXOM_APPROVAL_VERIFY_KEY; then
  start signer "$ROOT/axom/target/debug/axom-signer"
  bind="${AXOM_SIGNER_BIND:-127.0.0.1:8090}"
  export AXOM_SIGNER_URL="${AXOM_SIGNER_URL:-http://127.0.0.1:${bind##*:}}"
else
  echo "  skipping signer: set OPENBAO_ADDR, OPENBAO_TOKEN, and both *_VERIFY_KEY (payouts wait until it runs)"
  unset AXOM_SIGNER_URL
fi
start axom "$ROOT/axom/target/debug/axom"

if need LIVEKIT_API_KEY && need LIVEKIT_API_SECRET && command -v axom-meet > /dev/null; then
  start meet-agent axom-meet start
else
  echo "  skipping meeting agent: needs LIVEKIT_* and 'pip install -e meet-agent' (interviews wait until it runs)"
fi

if command -v npm > /dev/null; then
  [[ -d admin/node_modules ]] || (cd admin && npm ci --silent)
  [[ -d meet-web/node_modules ]] || (cd meet-web && npm ci --silent)
  # Both apps call the API through their own /api proxy (vite.config.js), so
  # VITE_AXOM_API_URL in .env (meant for Docker builds) can't point them at
  # the wrong place, and no CORS setup is needed.
  api_port="${AXOM_API_BIND:-127.0.0.1:8080}"; api_port="${api_port##*:}"
  export VITE_AXOM_API_URL=/api AXOM_API_PROXY_TARGET="http://127.0.0.1:$api_port"
  # exec the local vite binary so stopping it stops the server itself
  # (through npx it runs in a child process that would be left behind).
  start dashboard bash -c "cd admin && exec ./node_modules/.bin/vite --port 5174 --strictPort"
  start candidate-site bash -c "cd meet-web && exec ./node_modules/.bin/vite --port 5173 --strictPort"
else
  echo "  skipping web apps: npm not found"
fi

sleep 3
echo
echo "Running. Open:"
echo "  Dashboard:        http://localhost:5174"
echo "  Jobs board:       http://localhost:5173/jobs"
echo "  API health:       http://localhost:8080/health"
echo "Ctrl+C stops everything."
# Wait until any part exits (or Ctrl+C), then stop the rest.
while :; do
  for pid in ${PIDS[@]+"${PIDS[@]}"}; do
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "A process exited; see .dev-logs/. Stopping the rest."
      exit 1
    fi
  done
  sleep 2
done
