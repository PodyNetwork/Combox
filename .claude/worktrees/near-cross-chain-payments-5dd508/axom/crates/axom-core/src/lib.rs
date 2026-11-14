//! ForeMan core domain: plans, org chart, workers, tasks, payouts, state
//! machines, the append-only audit log, and the untrusted-data wrapper.

pub mod audit;
pub mod authz;
pub mod db;
pub mod decision;
pub mod domain;
pub mod intents;
pub mod near;
pub mod rhea;
pub mod secretbox;
pub mod state;
pub mod untrusted;

pub use rust_decimal::Decimal;
pub use uuid::Uuid;

/// Embedded migrations from `/supabase/migrations`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../../supabase/migrations");
