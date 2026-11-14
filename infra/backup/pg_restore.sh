#!/usr/bin/env bash
# Restore a pg_backup.sh dump into an EMPTY database.
#   TARGET_DATABASE_URL=... ./pg_restore.sh axom-YYYYmmddTHHMMSSZ.dump[.age]
set -euo pipefail
: "${TARGET_DATABASE_URL:?}"
IN="${1:?dump file}"
if [[ "$IN" == *.age ]]; then
  : "${BACKUP_AGE_IDENTITY:?age identity file required to decrypt}"
  TMP="$(mktemp)"; age -d -i "$BACKUP_AGE_IDENTITY" -o "$TMP" "$IN"; IN="$TMP"
fi
pg_restore --no-owner --exit-on-error --dbname="$TARGET_DATABASE_URL" "$IN"
