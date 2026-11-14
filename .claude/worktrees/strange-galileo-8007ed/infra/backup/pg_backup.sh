#!/usr/bin/env bash
# Encrypted Postgres backup (custom format). Run nightly from Coolify's
# scheduled tasks. Encrypts with `age` to BACKUP_AGE_RECIPIENT when set.
#   DATABASE_URL=... BACKUP_DIR=/backups ./pg_backup.sh
set -euo pipefail
: "${DATABASE_URL:?}" "${BACKUP_DIR:=./backups}"
mkdir -p "$BACKUP_DIR"
OUT="$BACKUP_DIR/axom-$(date -u +%Y%m%dT%H%M%SZ).dump"
pg_dump --format=custom --no-owner --dbname="$DATABASE_URL" --file="$OUT"
if [[ -n "${BACKUP_AGE_RECIPIENT:-}" ]]; then
  age -r "$BACKUP_AGE_RECIPIENT" -o "$OUT.age" "$OUT" && rm -f "$OUT" && OUT="$OUT.age"
fi
echo "$OUT"
