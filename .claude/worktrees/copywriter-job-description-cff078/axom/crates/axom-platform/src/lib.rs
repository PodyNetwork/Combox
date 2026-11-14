//! Multi-tenant platform: anyone can sign up a company, which gets its own
//! database, encrypted integration keys, members, and a subscription paid in
//! NEAR. See `http` for the routes and DECISIONS.md (MT-xx) for the design.

pub mod billing;
pub mod crypto;
pub mod http;
pub mod mail;
pub mod operator;
pub mod secrets;
pub mod tenants;
pub mod treasury;

pub use tenants::{Platform, Subscription, Tenant, TenantError};

/// Migrations for the platform database (separate from the per-company
/// migrations in `axom_core::MIGRATOR`).
pub static PLATFORM_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../../supabase/platform_migrations");
