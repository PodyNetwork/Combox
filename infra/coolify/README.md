# Coolify resources

One Coolify "Docker Compose" resource per file. Order:

1. **Supabase** (Postgres + Auth): use Coolify's one-click Supabase service (or Supabase's official self-hosting compose). Enable TOTP MFA in Auth settings. Combox's tables live in the `axom` schema and are migrated automatically on start.
2. **openbao.yml**: first put OpenBao's TLS certificate and key in `infra/coolify/tls/openbao/` as `tls.crt` and `tls.key` (the certificate must name `openbao.internal`). OpenBao runs as the image's `openbao` user (uid 100, gid 1000), so make the key readable by it and nobody else:
   ```bash
   chown 100:1000 infra/coolify/tls/openbao/tls.key && chmod 600 infra/coolify/tls/openbao/tls.key
   ```
   Then deploy, initialize (`bao operator init`), unseal (`bao operator unseal`, after every restart), and run `infra/openbao/setup-treasury.sh` (owner only). Set the printed signer token as `OPENBAO_TOKEN` (and, multi-tenant, the provisioner token as `OPENBAO_PROVISIONER_TOKEN`) in the Combox resource's environment editor. Never save either to a file. Raft data lives in the `openbao-data` volume (`/openbao/file`) and the audit log in `openbao-audit` (`/openbao/logs/audit.log`); back up with `infra/backup/openbao_snapshot.sh`.
3. **docuseal.yml** (contracts). The CRM for meeting notes is opendesk: its hosted service (opendesk.sh), or self-hosted with its own Docker setup (github.com/useopendesk/opendesk). **postiz.yml** only if your AI workers will use Postiz through its MCP server (connect it on the dashboard's Tools page).
4. **axom.yml**: Combox, the signer, LiveKit (video meetings, host networking, TURN), the meeting agent, and the two web apps. Deploy it once for dev and once for prod.

These definitions pin image versions. `openbao.yml` has been started, initialized and unsealed with Docker (DECISIONS M9-11); the others were not deployed from the build environment. Validate each against the service's own self-hosting docs on first deploy.
