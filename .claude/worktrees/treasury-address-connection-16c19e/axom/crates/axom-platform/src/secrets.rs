//! Each company's integration settings and API keys, encrypted at rest.
//!
//! Only names in [`CATALOG`] can be stored. Values marked `secret` are never
//! returned by the API, only a hint (last four characters).

use std::collections::HashMap;

use axom_hr::approvals::AdminSession;
use axom_tools::config::MapSource;
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::tenants::{audit, Platform, TenantError};

pub use axom_tools::integrations::{Field, Integration, CATALOG};
use axom_tools::integrations::{field, validate};

/// Settings a company may leave unset to use the platform's shared service.
pub const SHARED_KEYS: &[&str] = &[
    "SMTP_HOST", "SMTP_PORT", "SMTP_USERNAME", "SMTP_PASSWORD", "SMTP_FROM",
];

/// Shared settings a company may set on its own while still using the
/// platform's service: they carry no credentials.
pub const SHARED_STANDALONE: &[&str] = &["SMTP_FROM"];

/// A company's settings with the platform's shared services as fallback
/// (owned, for long-running company runtimes).
pub struct CompanyConfig(pub MapSource);

impl axom_tools::config::ConfigSource for CompanyConfig {
    fn get(&self, key: &str) -> Option<String> {
        axom_tools::config::Layered { own: &self.0, fallback: &axom_tools::config::EnvSource, shared_keys: SHARED_KEYS, standalone_keys: SHARED_STANDALONE }.get(key)
    }

    fn tenant_supplied(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredValue {
    pub name: String,
    /// The value for non-secret settings; `None` for secrets.
    pub value: Option<String>,
    /// Last four characters, for recognising a secret.
    pub hint: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Platform {
    async fn require_member(&self, tenant: Uuid, s: &AdminSession) -> Result<(), TenantError> {
        if !s.mfa_verified {
            return Err(TenantError::Forbidden("an MFA-verified session is required".into()));
        }
        match self.role(tenant, s.user_id).await? {
            Some(_) => Ok(()),
            None => Err(TenantError::Forbidden("not a member of this company".into())),
        }
    }

    pub async fn set_secret(&self, tenant: Uuid, s: &AdminSession, name: &str, value: &str) -> Result<(), TenantError> {
        self.require_member(tenant, s).await?;
        let (f, value) = validate(name, value).map_err(TenantError::Invalid)?;
        // Service addresses must be public https hosts. A host that doesn't
        // resolve right now may be saved; it is checked again on every call.
        if name.ends_with("_URL") {
            if let Err(axom_tools::ToolError::Permanent(why)) = axom_tools::adapters::mcp::UrlGuard::strict().check(value).await {
                return Err(TenantError::Invalid(why));
            }
        }
        let (nonce, ct) = self.secrets.seal(tenant, name, value);
        sqlx::query(
            "INSERT INTO platform.secrets (tenant_id, name, nonce, ciphertext, hint, updated_by) VALUES ($1,$2,$3,$4,$5,$6)
             ON CONFLICT (tenant_id, name) DO UPDATE SET nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext, hint = EXCLUDED.hint,
               updated_by = EXCLUDED.updated_by, updated_at = now()",
        )
        .bind(tenant)
        .bind(name)
        .bind(nonce)
        .bind(ct)
        .bind(axom_tools::integrations::display(f, value))
        .bind(s.user_id)
        .execute(&self.db)
        .await?;
        // The audit log records which setting changed, never its value.
        audit(&self.db, Some(tenant), &format!("user:{}", s.user_id), "integration.set", json!({ "name": name })).await?;
        Ok(())
    }

    pub async fn delete_secret(&self, tenant: Uuid, s: &AdminSession, name: &str) -> Result<(), TenantError> {
        self.require_member(tenant, s).await?;
        sqlx::query("DELETE FROM platform.secrets WHERE tenant_id = $1 AND name = $2").bind(tenant).bind(name).execute(&self.db).await?;
        audit(&self.db, Some(tenant), &format!("user:{}", s.user_id), "integration.delete", json!({ "name": name })).await?;
        Ok(())
    }

    pub async fn list_secrets(&self, tenant: Uuid, s: &AdminSession) -> Result<Vec<StoredValue>, TenantError> {
        self.require_member(tenant, s).await?;
        let rows: Vec<(String, String, chrono::DateTime<chrono::Utc>)> =
            sqlx::query_as("SELECT name, hint, updated_at FROM platform.secrets WHERE tenant_id = $1 ORDER BY name").bind(tenant).fetch_all(&self.db).await?;
        Ok(rows
            .into_iter()
            .map(|(name, h, updated_at)| {
                let secret = field(&name).is_none_or(|f| f.secret);
                StoredValue { value: (!secret).then(|| h.clone()), hint: if secret { h } else { String::new() }, name, updated_at }
            })
            .collect())
    }

    /// All of a company's settings, decrypted, for building its adapters.
    /// Values that fail to decrypt are skipped (and logged).
    pub async fn config(&self, tenant: Uuid) -> Result<MapSource, sqlx::Error> {
        let rows: Vec<(String, Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT name, nonce, ciphertext FROM platform.secrets WHERE tenant_id = $1").bind(tenant).fetch_all(&self.db).await?;
        let mut map = HashMap::new();
        for (name, nonce, ct) in rows {
            match self.secrets.open(tenant, &name, &nonce, &ct) {
                Ok(v) => {
                    map.insert(name, v);
                }
                Err(e) => tracing::error!(%tenant, %name, error = %e, "stored setting could not be decrypted"),
            }
        }
        Ok(MapSource(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_keys_are_in_the_catalog() {
        assert!(SHARED_KEYS.iter().all(|k| field(k).is_some()));
    }
}
