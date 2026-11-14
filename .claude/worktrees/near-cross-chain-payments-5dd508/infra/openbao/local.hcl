# OpenBao for your own Combox in Docker (docker-compose.yml, payouts
# profile). ./scripts/setup initializes it and creates the treasury key;
# ./scripts/start unseals it. Storage and the audit log live in Docker
# volumes, in the folders the image makes writable.
ui            = false
disable_mlock = true

storage "raft" {
  path    = "/openbao/file"
  node_id = "openbao-local"
}

# Plain HTTP on the Compose network only: no port is published to the host.
listener "tcp" {
  address     = "0.0.0.0:8200"
  tls_disable = true
}

api_addr     = "http://openbao:8200"
cluster_addr = "http://openbao:8201"

# Every request, including every sign request, is written here.
# OpenBao 2.x only accepts audit devices declared in config.
audit "file" "file" {
  options {
    file_path = "/openbao/logs/audit.log"
  }
}
