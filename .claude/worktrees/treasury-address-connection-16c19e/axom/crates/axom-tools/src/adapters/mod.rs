//! Real adapters for external services. Every adapter:
//! - is dry-run by default (records the request, makes no call);
//! - refuses to act without an `Authorized` from an allow decision for an
//!   action it serves;
//! - maps vendor errors to retryable / permanent / rate-limited;
//! - reads credentials from the environment and redacts them from errors.
//!
//! LiveKit is in `livekit` (milestone 6) and Upwork in `upwork` (milestone 8).

pub mod docuseal;
pub mod http;
pub mod livekit;
pub mod mcp;
pub mod opendesk;
pub mod postiz;
pub mod smtp;
pub mod upwork;
pub mod warmbly;
