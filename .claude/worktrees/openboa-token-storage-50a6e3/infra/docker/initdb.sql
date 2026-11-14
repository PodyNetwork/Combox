-- Runs once, when the local Docker database is first created. The
-- multi-tenant platform keeps its own control-plane database apart from the
-- single-company one (see docker-compose.yml and docs/SETUP.md).
CREATE DATABASE axom_platform;
