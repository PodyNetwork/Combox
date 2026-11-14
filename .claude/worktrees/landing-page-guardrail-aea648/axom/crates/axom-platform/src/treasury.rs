//! Each company's own NEAR treasury: a non-exportable, undeletable ed25519
//! key in OpenBao named `axom-treasury-<id>`. The company funds its implicit
//! account; Combox never pools customer money. Only axom-signer can sign with
//! it; the platform's provisioner token can create keys but not use them.

use async_trait::async_trait;
use axom_core::near::PublicKey;
use axom_hr::approvals::AdminSession;
use serde_json::json;
use uuid::Uuid;

use crate::tenants::{audit, Platform, TenantError};

#[async_trait]
pub trait TreasuryKeys: Send + Sync {
    /// Create (or return) the key and its public key.
    async fn create(&self, key_name: &str) -> anyhow::Result<PublicKey>;
}

/// OpenBao with the provisioner policy (`OPENBAO_PROVISIONER_TOKEN`; the
/// token is never read from a file).
pub struct OpenBaoProvisioner {
    pub addr: String,
    pub token: String,
}

impl OpenBaoProvisioner {
    pub fn from_env() -> Option<Self> {
        let addr = std::env::var("OPENBAO_ADDR").ok().filter(|v| !v.is_empty())?;
        if std::env::var_os("OPENBAO_PROVISIONER_TOKEN_FILE").is_some_and(|v| !v.is_empty()) {
            tracing::error!("OPENBAO_PROVISIONER_TOKEN_FILE is no longer supported: set OPENBAO_PROVISIONER_TOKEN and delete the token file");
            return None;
        }
        let token = std::env::var("OPENBAO_PROVISIONER_TOKEN").ok()?.trim().to_string();
        // Empty (single-company deployments) means "not set".
        (!token.is_empty()).then_some(Self { addr, token })
    }
}

#[async_trait]
impl TreasuryKeys for OpenBaoProvisioner {
    async fn create(&self, key_name: &str) -> anyhow::Result<PublicKey> {
        let t = axom_signer::openbao::OpenBaoTransit::new(&self.addr, &self.token, key_name);
        Ok(t.create_treasury_key().await?)
    }
}

pub fn key_name(tenant: Uuid) -> String {
    format!("axom-treasury-{}", &tenant.simple().to_string()[..16])
}

impl Platform {
    /// Set up the company's treasury (owner only). Idempotent: returns the
    /// existing account if already set up.
    pub async fn setup_treasury(&self, tenant: Uuid, s: &AdminSession, keys: &dyn TreasuryKeys) -> Result<String, TenantError> {
        if !s.mfa_verified || self.role(tenant, s.user_id).await?.as_deref() != Some("owner") {
            return Err(TenantError::Forbidden("only the company owner can set up the treasury".into()));
        }
        let t = self.by_id(tenant).await?;
        if let Some(a) = t.treasury_account {
            return Ok(a);
        }
        let name = key_name(tenant);
        let pk = keys.create(&name).await.map_err(|e| TenantError::Provision(format!("treasury key: {e}")))?;
        let account = pk.implicit_account();
        sqlx::query("UPDATE platform.tenants SET treasury_key = $2, treasury_account = $3 WHERE id = $1 AND treasury_key IS NULL")
            .bind(tenant)
            .bind(&name)
            .bind(&account)
            .execute(&self.db)
            .await?;
        audit(&self.db, Some(tenant), &format!("user:{}", s.user_id), "treasury.create", json!({ "key": name, "account": account })).await?;
        Ok(account)
    }

    /// Companies with a treasury, for the signer: (company, key name).
    pub async fn treasuries(&self) -> Result<Vec<(crate::Tenant, String)>, sqlx::Error> {
        Ok(self.active_tenants().await?.into_iter().filter_map(|t| t.treasury_key.clone().map(|k| (t, k))).collect())
    }
}
