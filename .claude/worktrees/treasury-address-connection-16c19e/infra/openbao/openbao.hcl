# OpenBao server config for the VPS (Coolify resource). Not dev mode: after
# a reboot OpenBao stays sealed until the owner unseals it, and payouts pause
# in the meantime. That is the intended failure mode.
ui = false

# Storage and the audit log use the folders the image's entrypoint makes
# writable for the openbao user (/openbao/file, /openbao/logs); the Coolify
# resource mounts its volumes there.

storage "raft" {
  path    = "/openbao/file"
  node_id = "openbao-1"
}

listener "tcp" {
  address       = "0.0.0.0:8200"
  tls_cert_file = "/openbao/tls/tls.crt"
  tls_key_file  = "/openbao/tls/tls.key"
}

api_addr     = "https://openbao.internal:8200"
cluster_addr = "https://openbao.internal:8201"

# Every request, including every sign request, is written here.
# OpenBao 2.x only accepts audit devices declared in config.
audit "file" "file" {
  options {
    file_path = "/openbao/logs/audit.log"
  }
}
