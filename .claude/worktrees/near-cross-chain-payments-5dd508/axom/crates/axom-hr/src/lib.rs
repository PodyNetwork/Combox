//! HR services: the deterministic policy engine, the monthly cap ledger,
//! payouts, owner notifications, and the HR agent's proposal gate.

pub mod agent;
pub mod approvals;
pub mod board;
pub mod cap;
pub mod company_policy;
pub mod custom_tools;
pub mod destinations;
pub mod firing;
pub mod gates;
pub mod hiring;
pub mod meetings;
pub mod notify;
pub mod payouts;
pub mod payroll;
pub mod performance;
pub mod policy;
pub mod privacy;
pub mod reviews;
pub mod routing;
pub mod treasury;

pub use policy::{Action, Context, Decision, Outcome, PolicyConfig, PriceQuote};
