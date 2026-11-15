#!/usr/bin/env bash
# Upload a Combox contract template to DocuSeal and print its id for
# DOCUSEAL_TEMPLATES.
#
#   COMPANY_NAME="Acme Ltd" GOVERNING_LAW="England and Wales" \
#     ./infra/docuseal/upload-template.sh [freelancer-services-v1]
#
# Needs DOCUSEAL_URL and DOCUSEAL_API_KEY (read from .env if present).
# Have the owner read the uploaded template in DocuSeal before adding its id
# to DOCUSEAL_TEMPLATES: only templates listed there can be sent.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
NAME="${1:-freelancer-services-v1}"
FILE="$ROOT/infra/docuseal/$NAME.html"

# .env fills only what the command line did not set.
if [[ -f "$ROOT/.env" ]]; then
  given_url="${DOCUSEAL_URL:-}" given_key="${DOCUSEAL_API_KEY:-}"
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/.env"
  set +a
  DOCUSEAL_URL="${given_url:-${DOCUSEAL_URL:-}}"
  DOCUSEAL_API_KEY="${given_key:-${DOCUSEAL_API_KEY:-}}"
fi

for v in DOCUSEAL_URL DOCUSEAL_API_KEY COMPANY_NAME GOVERNING_LAW; do
  if [[ -z "${!v:-}" ]]; then echo "Set $v first." >&2; exit 1; fi
done
[[ -f "$FILE" ]] || { echo "No template at $FILE" >&2; exit 1; }
command -v jq >/dev/null || { echo "Needs jq." >&2; exit 1; }

# Fill the company placeholders; refuse if any are left.
html="$(sed -e "s|__COMPANY_NAME__|${COMPANY_NAME//|/\\|}|g" \
            -e "s|__GOVERNING_LAW__|${GOVERNING_LAW//|/\\|}|g" "$FILE")"
if grep -q '__[A-Z_]*__' <<<"$html"; then
  echo "Unfilled placeholders remain in $FILE." >&2
  exit 1
fi

resp="$(jq -n --arg html "$html" --arg name "$NAME" --arg ext "$NAME" \
          '{html: $html, name: $name, external_id: $ext}' |
  curl -fsS -X POST "${DOCUSEAL_URL%/}/api/templates/html" \
    -H "X-Auth-Token: $DOCUSEAL_API_KEY" \
    -H "Content-Type: application/json" \
    --data-binary @-)"

id="$(jq -r '.id // empty' <<<"$resp")"
[[ -n "$id" ]] || { echo "DocuSeal gave no template id: $resp" >&2; exit 1; }
echo "Uploaded $NAME as DocuSeal template $id."
echo "After the owner approves it, add to DOCUSEAL_TEMPLATES:  $NAME=$id"
