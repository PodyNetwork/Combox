#!/usr/bin/env bash
# OpenBao raft snapshot (owner's admin token). The snapshot is encrypted by
# OpenBao's barrier: restoring it needs the ORIGINAL unseal key(s), so keep
# those offline and separate from the snapshot.
#   BAO_ADDR=... BAO_TOKEN=<owner admin> ./openbao_snapshot.sh save|restore <file>
set -euo pipefail
case "${1:?save|restore}" in
  save) bao operator raft snapshot save "${2:?file}" ;;
  restore) bao operator raft snapshot restore -force "${2:?file}" ;;
  *) echo "usage: $0 save|restore <file>" >&2; exit 2 ;;
esac
