# OpenBao policy for the multi-tenant platform's provisioner (the Combox API).
# It may create a company's treasury key and lock it (not exportable, not
# deletable), and read public keys. It can't sign, export, rotate, back up,
# or delete keys: signing stays with axom-signer only.
path "transit/keys/axom-treasury-*" {
  capabilities = ["create", "update", "read"]
}

path "transit/keys/axom-treasury-*/config" {
  capabilities = ["update"]
}
