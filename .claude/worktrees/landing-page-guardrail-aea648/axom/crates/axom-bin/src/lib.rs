//! Library side of the `axom` binary: seed data and the judge-mode scenario.

pub mod demo;
pub mod multi;
pub mod runtime;
pub mod single;

/// Base URL the meeting agent uses to reach this API (its consent and result
/// callbacks, and the company's interview keys): `AXOM_AGENT_CALLBACK_URL`,
/// else `AXOM_PUBLIC_API_URL`. Set it when the agent runs somewhere the public
/// address doesn't lead back here: in Docker Compose, `localhost` inside the
/// agent's container is the agent itself, so it needs `http://axom:8080`.
pub fn agent_callback_url() -> String {
    ["AXOM_AGENT_CALLBACK_URL", "AXOM_PUBLIC_API_URL"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|u| !u.trim().is_empty()))
        .unwrap_or_else(|| "http://localhost:8080".into())
}

/// Why a signing-key variable can't be used, without revealing its value.
pub fn signing_key_problem(var: &str) -> String {
    let Ok(raw) = std::env::var(var) else { return format!("{var} is not set") };
    let v = raw.trim();
    if v.is_empty() {
        return format!("{var} is empty");
    }
    let hint = if v.starts_with("ed25519:") {
        " It looks like a VERIFY_KEY; use the SIGNING_KEY line instead."
    } else if v.contains('=') {
        " It contains '=': paste only the part after SIGNING_KEY=."
    } else if v.starts_with('"') || v.starts_with('\'') {
        " Remove the quotes around it."
    } else {
        ""
    };
    format!("{var} must be the SIGNING_KEY value printed by `axom keygen`: 64 characters, 0-9 and a-f (this one has {} characters).{hint}", v.chars().count())
}
