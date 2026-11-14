-- Treasury yield on Rhea lending becomes a company setting (Settings page).
-- NULL uses the platform's default ([treasury] yield_enabled in
-- config/policy.toml, or TREASURY_YIELD_ENABLED=true); true or false is the
-- company's own choice. Changed only by an MFA admin, audited like the rest
-- of the company's rules.
ALTER TABLE axom.company_policy ADD COLUMN treasury_yield_enabled BOOLEAN;
