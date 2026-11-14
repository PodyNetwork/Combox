# OpenBao policy for axom-signer. It may sign with treasury keys and read
# their public keys, and nothing else: no key creation, rotation, export,
# deletion, or config changes. `axom-treasury` is the single-company key;
# `axom-treasury-*` are the multi-tenant platform's per-company keys.
path "transit/sign/axom-treasury" {
  capabilities = ["update"]
}

path "transit/keys/axom-treasury" {
  capabilities = ["read"]
}

path "transit/sign/axom-treasury-*" {
  capabilities = ["update"]
}

path "transit/keys/axom-treasury-*" {
  capabilities = ["read"]
}

# The signer's token is periodic: it must renew itself or it expires one
# period (32 days) after setup, and payouts stop. This lets it extend its own
# life and nothing else.
path "auth/token/renew-self" {
  capabilities = ["update"]
}
