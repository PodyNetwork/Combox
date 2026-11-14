//! Companies (tenants): sign-up, their own database, members and invites.
//!
//! Each company gets its own Postgres database, created from the same
//! migrations as a single-company deployment, so every existing HR, payout,
//! and hiring query runs unchanged and can't see another company's rows.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axom_hr::approvals::AdminSession;
use axom_hr::PolicyConfig;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::billing::PlanConfig;
use crate::crypto::SecretBox;

/// Most companies one person can own (blunts abuse of free trials).
pub const MAX_OWNED_COMPANIES: i64 = 5;

/// Slugs that would collide with routes or confuse people.
const RESERVED: &[&str] = &["admin", "api", "app", "platform", "www", "t", "health", "jobs", "mcp", "billing", "signup", "login"];

#[derive(Debug, thiserror::Error)]
pub enum TenantError {
    #[error("{0}")]
    Invalid(String),
    #[error("that address is taken; choose another")]
    SlugTaken,
    #[error("not allowed: {0}")]
    Forbidden(String),
    #[error("company not found")]
    NotFound,
    #[error("setting up the company database failed: {0}")]
    Provision(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Tenant {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    #[serde(skip)]
    pub db_name: String,
    pub status: String,
    pub trial_ends_at: DateTime<Utc>,
    pub paid_until: Option<DateTime<Utc>>,
    pub treasury_account: Option<String>,
    #[serde(skip)]
    pub treasury_key: Option<String>,
    pub owner_email: String,
    pub created_at: DateTime<Utc>,
}

const TENANT_COLS: &str = "id, slug, name, db_name, status, trial_ends_at, paid_until, treasury_account, treasury_key, owner_email, created_at";

/// Subscription state at a moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Subscription {
    Trial { ends_at: DateTime<Utc> },
    Active { paid_until: DateTime<Utc> },
    Expired { since: DateTime<Utc> },
}

impl Subscription {
    pub fn is_usable(&self) -> bool {
        !matches!(self, Subscription::Expired { .. })
    }
}

impl Tenant {
    pub fn subscription(&self, now: DateTime<Utc>) -> Subscription {
        match self.paid_until {
            Some(p) if p > now => Subscription::Active { paid_until: p },
            _ if self.trial_ends_at > now => Subscription::Trial { ends_at: self.trial_ends_at },
            Some(p) => Subscription::Expired { since: p.max(self.trial_ends_at) },
            None => Subscription::Expired { since: self.trial_ends_at },
        }
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Member {
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

/// The control plane: the platform database plus a pool per company.
pub struct Platform {
    pub db: PgPool,
    base: PgConnectOptions,
    pub secrets: SecretBox,
    pub policy: Arc<PolicyConfig>,
    pub plan: PlanConfig,
    pools: RwLock<HashMap<Uuid, PgPool>>,
}

pub fn validate_slug(slug: &str) -> Result<(), TenantError> {
    let ok_chars = slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let len_ok = (3..=40).contains(&slug.len());
    let edges_ok = !slug.starts_with('-') && !slug.ends_with('-');
    if !(ok_chars && len_ok && edges_ok) {
        return Err(TenantError::Invalid("address: 3 to 40 lowercase letters, digits, or dashes (not at the ends)".into()));
    }
    if RESERVED.contains(&slug) {
        return Err(TenantError::Invalid("that address is reserved; choose another".into()));
    }
    Ok(())
}

pub async fn audit(db: &PgPool, tenant: Option<Uuid>, actor: &str, action: &str, details: Value) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO platform.audit_log (tenant_id, actor, action, details) VALUES ($1,$2,$3,$4)")
        .bind(tenant)
        .bind(actor)
        .bind(action)
        .bind(details)
        .execute(db)
        .await?;
    Ok(())
}

fn actor(s: &AdminSession) -> String {
    format!("user:{}", s.user_id)
}

impl Platform {
    /// `db` is the platform database; company databases are created on the
    /// same server with the same credentials (the role needs CREATEDB).
    pub async fn new(db: PgPool, secrets: SecretBox, policy: Arc<PolicyConfig>, plan: PlanConfig) -> anyhow::Result<Self> {
        crate::PLATFORM_MIGRATOR.run(&db).await?;
        let base = (*db.connect_options()).clone();
        Ok(Self { db, base, secrets, policy, plan, pools: RwLock::default() })
    }

    /// The company's database pool (created lazily, small).
    pub async fn pool(&self, t: &Tenant) -> PgPool {
        if let Some(p) = self.pools.read().await.get(&t.id) {
            return p.clone();
        }
        let opts = self.base.clone().database(&t.db_name).options([("search_path", "public")]);
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .min_connections(0)
            .idle_timeout(Duration::from_secs(600))
            .acquire_timeout(Duration::from_secs(10))
            .connect_lazy_with(opts);
        let mut pools = self.pools.write().await;
        if let Some(p) = pools.get(&t.id) {
            return p.clone();
        }
        pools.insert(t.id, pool.clone());
        drop(pools);
        // Bring the company's database up to date with any newer migrations
        // (companies created before an upgrade). A brand-new database is
        // migrated by `provision`; running it again is a no-op.
        if t.status != "provisioning" {
            if let Err(e) = axom_core::MIGRATOR.run(&pool).await {
                tracing::error!(company = %t.slug, error = %e, "migrating company database failed");
            }
        }
        pool
    }

    pub async fn by_slug(&self, slug: &str) -> Result<Tenant, TenantError> {
        sqlx::query_as::<_, Tenant>(&format!("SELECT {TENANT_COLS} FROM platform.tenants WHERE slug = $1 AND status <> 'provisioning'"))
            .bind(slug)
            .fetch_optional(&self.db)
            .await?
            .ok_or(TenantError::NotFound)
    }

    pub async fn by_id(&self, id: Uuid) -> Result<Tenant, TenantError> {
        sqlx::query_as::<_, Tenant>(&format!("SELECT {TENANT_COLS} FROM platform.tenants WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.db)
            .await?
            .ok_or(TenantError::NotFound)
    }

    /// Companies whose automation should run (set up and not suspended).
    pub async fn active_tenants(&self) -> Result<Vec<Tenant>, sqlx::Error> {
        sqlx::query_as::<_, Tenant>(&format!("SELECT {TENANT_COLS} FROM platform.tenants WHERE status = 'active' ORDER BY created_at"))
            .fetch_all(&self.db)
            .await
    }

    /// The caller's role in the company, if a member.
    pub async fn role(&self, tenant: Uuid, user: Uuid) -> Result<Option<String>, sqlx::Error> {
        let r: Option<(String,)> = sqlx::query_as("SELECT role FROM platform.members WHERE tenant_id = $1 AND user_id = $2")
            .bind(tenant)
            .bind(user)
            .fetch_optional(&self.db)
            .await?;
        Ok(r.map(|r| r.0))
    }

    /// Companies the user belongs to, after accepting any open invites for
    /// their (MFA-verified) email.
    pub async fn memberships(&self, s: &AdminSession) -> Result<Vec<(Tenant, String)>, TenantError> {
        if s.mfa_verified && !s.email.is_empty() {
            self.accept_invites(s).await?;
        }
        let rows: Vec<(Uuid, String)> = sqlx::query_as("SELECT tenant_id, role FROM platform.members WHERE user_id = $1").bind(s.user_id).fetch_all(&self.db).await?;
        let mut out = Vec::new();
        for (id, role) in rows {
            let t = self.by_id(id).await?;
            if t.status != "provisioning" {
                out.push((t, role));
            }
        }
        out.sort_by_key(|a| a.0.name.to_lowercase());
        Ok(out)
    }

    async fn accept_invites(&self, s: &AdminSession) -> Result<(), TenantError> {
        let open: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT id, tenant_id FROM platform.invites WHERE lower(email) = lower($1) AND accepted_at IS NULL AND revoked_at IS NULL",
        )
        .bind(&s.email)
        .fetch_all(&self.db)
        .await?;
        for (invite, tenant_id) in open {
            let t = self.by_id(tenant_id).await?;
            let name = display_name(&s.email);
            let pool = self.pool(&t).await;
            sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,$2,$3) ON CONFLICT (user_id) DO UPDATE SET active = true, email = EXCLUDED.email")
                .bind(s.user_id)
                .bind(&s.email)
                .bind(&name)
                .execute(&pool)
                .await?;
            sqlx::query("INSERT INTO platform.members (tenant_id, user_id, email, name, role) VALUES ($1,$2,$3,$4,'admin') ON CONFLICT DO NOTHING")
                .bind(tenant_id)
                .bind(s.user_id)
                .bind(&s.email)
                .bind(&name)
                .execute(&self.db)
                .await?;
            sqlx::query("UPDATE platform.invites SET accepted_at = now() WHERE id = $1").bind(invite).execute(&self.db).await?;
            audit(&self.db, Some(tenant_id), &actor(s), "member.invite_accepted", json!({ "invite_id": invite })).await?;
        }
        Ok(())
    }

    /// Sign up a company: its own database, migrations, the monthly cap,
    /// and the creator as owner and first admin. Starts the free trial.
    pub async fn create_tenant(&self, s: &AdminSession, name: &str, slug: &str, now: DateTime<Utc>) -> Result<Tenant, TenantError> {
        if !s.mfa_verified {
            return Err(TenantError::Forbidden("set up two-factor authentication before creating a company".into()));
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
            return Err(TenantError::Invalid("company name: 1 to 120 characters".into()));
        }
        let slug = slug.trim().to_lowercase();
        validate_slug(&slug)?;
        let (owned,): (i64,) = sqlx::query_as("SELECT count(*) FROM platform.members WHERE user_id = $1 AND role = 'owner'").bind(s.user_id).fetch_one(&self.db).await?;
        if owned >= MAX_OWNED_COMPANIES {
            return Err(TenantError::Forbidden(format!("you already own {MAX_OWNED_COMPANIES} companies")));
        }
        let id = Uuid::new_v4();
        let db_name = format!("axom_t_{}", &id.simple().to_string()[..16]);
        let trial_ends = now + chrono::Duration::days(self.plan.trial_days);
        let inserted = sqlx::query(
            "INSERT INTO platform.tenants (id, slug, name, db_name, trial_ends_at, owner_email, created_by) VALUES ($1,$2,$3,$4,$5,$6,$7)
             ON CONFLICT (slug) DO NOTHING",
        )
        .bind(id)
        .bind(&slug)
        .bind(name)
        .bind(&db_name)
        .bind(trial_ends)
        .bind(&s.email)
        .bind(s.user_id)
        .execute(&self.db)
        .await?;
        if inserted.rows_affected() == 0 {
            return Err(TenantError::SlugTaken);
        }
        match self.provision(id, &db_name, s).await {
            Ok(()) => {}
            Err(e) => {
                tracing::error!(error = %e, tenant = %id, "provisioning failed; rolling back");
                self.pools.write().await.remove(&id);
                let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)")).execute(&self.db).await;
                let _ = sqlx::query("DELETE FROM platform.tenants WHERE id = $1").bind(id).execute(&self.db).await;
                return Err(TenantError::Provision(e.to_string()));
            }
        }
        sqlx::query("INSERT INTO platform.members (tenant_id, user_id, email, name, role) VALUES ($1,$2,$3,$4,'owner')")
            .bind(id)
            .bind(s.user_id)
            .bind(&s.email)
            .bind(display_name(&s.email))
            .execute(&self.db)
            .await?;
        sqlx::query("UPDATE platform.tenants SET status = 'active' WHERE id = $1").bind(id).execute(&self.db).await?;
        audit(&self.db, Some(id), &actor(s), "company.create", json!({ "slug": slug, "name": name, "trial_ends_at": trial_ends })).await?;
        self.by_id(id).await
    }

    async fn provision(&self, id: Uuid, db_name: &str, s: &AdminSession) -> anyhow::Result<()> {
        // db_name is generated from a UUID (hex only), never from user input.
        debug_assert!(db_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        sqlx::query(&format!("CREATE DATABASE \"{db_name}\"")).execute(&self.db).await?;
        let t = self.by_id(id).await?;
        let pool = self.pool(&t).await;
        axom_core::MIGRATOR.run(&pool).await?;
        axom_hr::cap::seed_cap(&pool, &self.policy).await?;
        sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,$2,$3)")
            .bind(s.user_id)
            .bind(&s.email)
            .bind(display_name(&s.email))
            .execute(&pool)
            .await?;
        Ok(())
    }

    pub async fn members(&self, tenant: Uuid) -> Result<(Vec<Member>, Vec<Value>), sqlx::Error> {
        let members = sqlx::query_as::<_, Member>("SELECT user_id, email, name, role, created_at FROM platform.members WHERE tenant_id = $1 ORDER BY created_at")
            .bind(tenant)
            .fetch_all(&self.db)
            .await?;
        let invites: Vec<(Uuid, String, DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, email, created_at FROM platform.invites WHERE tenant_id = $1 AND accepted_at IS NULL AND revoked_at IS NULL ORDER BY created_at",
        )
        .bind(tenant)
        .fetch_all(&self.db)
        .await?;
        Ok((members, invites.into_iter().map(|(id, email, at)| json!({ "id": id, "email": email, "created_at": at })).collect()))
    }

    async fn require_owner(&self, tenant: Uuid, s: &AdminSession) -> Result<(), TenantError> {
        if !s.mfa_verified {
            return Err(TenantError::Forbidden("an MFA-verified session is required".into()));
        }
        match self.role(tenant, s.user_id).await?.as_deref() {
            Some("owner") => Ok(()),
            _ => Err(TenantError::Forbidden("only the company owner can manage members".into())),
        }
    }

    pub async fn invite(&self, tenant: Uuid, s: &AdminSession, email: &str) -> Result<Uuid, TenantError> {
        self.require_owner(tenant, s).await?;
        let email = email.trim().to_lowercase();
        let valid = email.split_once('@').is_some_and(|(l, d)| !l.is_empty() && d.contains('.')) && email.len() <= 254 && !email.contains(char::is_whitespace);
        if !valid {
            return Err(TenantError::Invalid("a valid email address is required".into()));
        }
        let (already,): (i64,) = sqlx::query_as("SELECT count(*) FROM platform.members WHERE tenant_id = $1 AND lower(email) = $2").bind(tenant).bind(&email).fetch_one(&self.db).await?;
        if already > 0 {
            return Err(TenantError::Invalid("that person is already a member".into()));
        }
        let id = Uuid::new_v4();
        let r = sqlx::query("INSERT INTO platform.invites (id, tenant_id, email, invited_by) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(id)
            .bind(tenant)
            .bind(&email)
            .bind(s.user_id)
            .execute(&self.db)
            .await?;
        if r.rows_affected() == 0 {
            return Err(TenantError::Invalid("an invitation for that email is already open".into()));
        }
        audit(&self.db, Some(tenant), &actor(s), "member.invite", json!({ "invite_id": id })).await?;
        Ok(id)
    }

    pub async fn revoke_invite(&self, tenant: Uuid, s: &AdminSession, invite: Uuid) -> Result<(), TenantError> {
        self.require_owner(tenant, s).await?;
        sqlx::query("UPDATE platform.invites SET revoked_at = now() WHERE id = $1 AND tenant_id = $2 AND accepted_at IS NULL").bind(invite).bind(tenant).execute(&self.db).await?;
        audit(&self.db, Some(tenant), &actor(s), "member.invite_revoked", json!({ "invite_id": invite })).await?;
        Ok(())
    }

    pub async fn remove_member(&self, tenant: Uuid, s: &AdminSession, user: Uuid) -> Result<(), TenantError> {
        self.require_owner(tenant, s).await?;
        if self.role(tenant, user).await?.as_deref() == Some("owner") {
            return Err(TenantError::Forbidden("the owner can't be removed".into()));
        }
        let t = self.by_id(tenant).await?;
        sqlx::query("DELETE FROM platform.members WHERE tenant_id = $1 AND user_id = $2").bind(tenant).bind(user).execute(&self.db).await?;
        sqlx::query("UPDATE axom.admins SET active = false WHERE user_id = $1").bind(user).execute(&self.pool(&t).await).await?;
        audit(&self.db, Some(tenant), &actor(s), "member.remove", json!({ "user_id": user })).await?;
        Ok(())
    }
}

/// "ada.lovelace@x.com" -> "ada.lovelace".
fn display_name(email: &str) -> String {
    email.split('@').next().filter(|n| !n.is_empty()).unwrap_or("Admin").chars().take(60).collect()
}

/// Bring an existing single-company ForeMan database onto the platform.
pub struct Import<'a> {
    /// Name of the existing database, on the same server as the platform's.
    pub database: &'a str,
    pub slug: &'a str,
    pub name: &'a str,
    /// The owner's sign-in account (Supabase user id and email).
    pub owner_user_id: Uuid,
    pub owner_email: &'a str,
}

impl Platform {
    /// Register an existing single-company database as a company: it is
    /// brought up to date with migrations, keeps all its data, and gets the
    /// free trial. The owner becomes owner and admin; the database's other
    /// active admins become admins (same Supabase project assumed).
    /// Nothing is copied: the platform uses the database where it is.
    pub async fn import_tenant(&self, i: &Import<'_>, now: DateTime<Utc>) -> Result<Tenant, TenantError> {
        let slug = i.slug.trim().to_lowercase();
        validate_slug(&slug)?;
        let name = i.name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err(TenantError::Invalid("company name: 1 to 120 characters".into()));
        }
        let db = i.database.trim();
        if db.is_empty() || db.len() > 63 || !db.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(TenantError::Invalid("database name: letters, digits, and _ only".into()));
        }
        let email = i.owner_email.trim().to_lowercase();
        if !email.contains('@') {
            return Err(TenantError::Invalid("a valid owner email is required".into()));
        }
        let (exists,): (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)").bind(db).fetch_one(&self.db).await?;
        if !exists {
            return Err(TenantError::Invalid(format!("no database named {db} on this server")));
        }
        let (current,): (String,) = sqlx::query_as("SELECT current_database()").fetch_one(&self.db).await?;
        if db == current {
            return Err(TenantError::Invalid("that's the platform's own database".into()));
        }
        let (taken,): (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM platform.tenants WHERE db_name = $1)").bind(db).fetch_one(&self.db).await?;
        if taken {
            return Err(TenantError::Invalid(format!("{db} already belongs to a company")));
        }
        // Check it's an ForeMan database and bring it up to date before
        // registering anything.
        let opts = self.base.clone().database(db).options([("search_path", "public")]);
        let pool = PgPoolOptions::new().max_connections(2).connect_with(opts).await.map_err(|e| TenantError::Provision(format!("connecting to {db}: {e}")))?;
        let (is_axom,): (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'axom' AND table_name = 'workers')").fetch_one(&pool).await?;
        if !is_axom {
            pool.close().await;
            return Err(TenantError::Invalid(format!("{db} isn't an ForeMan database")));
        }
        axom_core::MIGRATOR.run(&pool).await.map_err(|e| TenantError::Provision(format!("migrating {db}: {e}")))?;
        sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1,$2,$3) ON CONFLICT (user_id) DO UPDATE SET active = true, email = EXCLUDED.email")
            .bind(i.owner_user_id)
            .bind(&email)
            .bind(display_name(&email))
            .execute(&pool)
            .await?;
        let admins: Vec<(Uuid, String, String)> = sqlx::query_as("SELECT user_id, email, name FROM axom.admins WHERE active AND user_id <> $1").bind(i.owner_user_id).fetch_all(&pool).await?;
        pool.close().await;

        let id = Uuid::new_v4();
        let trial_ends = now + chrono::Duration::days(self.plan.trial_days);
        let mut tx = self.db.begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO platform.tenants (id, slug, name, db_name, status, trial_ends_at, owner_email, created_by) VALUES ($1,$2,$3,$4,'active',$5,$6,$7)
             ON CONFLICT (slug) DO NOTHING",
        )
        .bind(id)
        .bind(&slug)
        .bind(name)
        .bind(db)
        .bind(trial_ends)
        .bind(&email)
        .bind(i.owner_user_id)
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() == 0 {
            return Err(TenantError::SlugTaken);
        }
        sqlx::query("INSERT INTO platform.members (tenant_id, user_id, email, name, role) VALUES ($1,$2,$3,$4,'owner')")
            .bind(id)
            .bind(i.owner_user_id)
            .bind(&email)
            .bind(display_name(&email))
            .execute(&mut *tx)
            .await?;
        for (user, e, n) in &admins {
            sqlx::query("INSERT INTO platform.members (tenant_id, user_id, email, name, role) VALUES ($1,$2,$3,$4,'admin') ON CONFLICT DO NOTHING")
                .bind(id)
                .bind(user)
                .bind(e)
                .bind(n)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("INSERT INTO platform.audit_log (tenant_id, actor, action, details) VALUES ($1,'cli:import-company','company.import',$2)")
            .bind(id)
            .bind(json!({ "slug": slug, "database": db, "owner": email, "admins": admins.len() }))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.by_id(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        for ok in ["acme", "acme-labs", "a1b", "x".repeat(40).as_str()] {
            assert!(validate_slug(ok).is_ok(), "{ok}");
        }
        for bad in ["ab", "-acme", "acme-", "Acme", "ac me", "acme_labs", "admin", "x".repeat(41).as_str(), "../x"] {
            assert!(validate_slug(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn subscription_states() {
        let now = Utc::now();
        let mut t = Tenant {
            id: Uuid::new_v4(), slug: "a".into(), name: "A".into(), db_name: "d".into(), status: "active".into(),
            trial_ends_at: now + chrono::Duration::days(3), paid_until: None, treasury_account: None, treasury_key: None,
            owner_email: "o@x.com".into(), created_at: now,
        };
        assert!(matches!(t.subscription(now), Subscription::Trial { .. }));
        t.trial_ends_at = now - chrono::Duration::days(1);
        assert!(!t.subscription(now).is_usable());
        t.paid_until = Some(now + chrono::Duration::days(30));
        assert!(matches!(t.subscription(now), Subscription::Active { .. }));
        t.paid_until = Some(now - chrono::Duration::hours(1));
        assert!(!t.subscription(now).is_usable());
    }
}
