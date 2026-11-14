//! Where adapters read their settings from: the process environment (one
//! company per deployment) or a company's stored integration keys
//! (multi-tenant platform).

use std::collections::HashMap;

pub trait ConfigSource: Send + Sync {
    /// The value, or `None` when unset or empty.
    fn get(&self, key: &str) -> Option<String>;

    /// Whether these values were typed in by a company on the multi-tenant
    /// platform. Service addresses from such a source are only ever
    /// contacted through the strict URL guard (public https hosts, no
    /// redirects), so a company can't aim the platform at its internals.
    fn tenant_supplied(&self) -> bool {
        false
    }
}

/// The process environment.
pub struct EnvSource;

impl ConfigSource for EnvSource {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }
}

/// A fixed set of values (e.g. a company's decrypted keys).
#[derive(Default, Clone)]
pub struct MapSource(pub HashMap<String, String>);

impl ConfigSource for MapSource {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).filter(|v| !v.is_empty()).cloned()
    }
}

/// A company's own values first; for the listed keys only, fall back to the
/// platform's environment (shared services such as LiveKit or SMTP).
///
/// The shared keys describe one service, and the fallback is all or nothing:
/// once a company sets any of them itself (its own host, port, username or
/// password), none of the others come from the platform. Otherwise a company
/// could point `SMTP_HOST` at its own server and be sent the platform's
/// username and password. `standalone_keys` are the exception: settings that
/// carry no credentials and may be set on their own while still using the
/// shared service (the From address).
pub struct Layered<'a> {
    pub own: &'a dyn ConfigSource,
    pub fallback: &'a dyn ConfigSource,
    pub shared_keys: &'a [&'a str],
    pub standalone_keys: &'a [&'a str],
}

impl Layered<'_> {
    /// The company runs its own instance of the shared service.
    fn own_service(&self) -> bool {
        self.shared_keys.iter().any(|k| !self.standalone_keys.contains(k) && self.own.get(k).is_some())
    }
}

impl ConfigSource for Layered<'_> {
    fn get(&self, key: &str) -> Option<String> {
        self.own.get(key).or_else(|| if self.shared_keys.contains(&key) && !self.own_service() { self.fallback.get(key) } else { None })
    }

    fn tenant_supplied(&self) -> bool {
        true
    }
}

/// One company per deployment: values set in the dashboard first, then the
/// environment (`.env`) for anything not set there.
#[derive(Default, Clone)]
pub struct WithEnv(pub MapSource);

impl ConfigSource for WithEnv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).or_else(|| EnvSource.get(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMTP: &[&str] = &["SMTP_HOST", "SMTP_PORT", "SMTP_USERNAME", "SMTP_PASSWORD", "SMTP_FROM"];

    fn platform() -> MapSource {
        MapSource(
            [("SMTP_HOST", "smtp.platform"), ("SMTP_USERNAME", "platform-user"), ("SMTP_PASSWORD", "platform-secret"), ("SMTP_FROM", "noreply@platform.example"), ("DOCUSEAL_API_KEY", "platform-key")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
        )
    }

    #[test]
    fn layered_only_falls_back_for_shared_keys() {
        let own = MapSource([("SMTP_FROM".to_string(), "hr@company.example".to_string()), ("EMPTY".into(), String::new())].into());
        let platform = platform();
        let c = Layered { own: &own, fallback: &platform, shared_keys: SMTP, standalone_keys: &["SMTP_FROM"] };
        assert_eq!(c.get("SMTP_FROM").as_deref(), Some("hr@company.example"));
        assert_eq!(c.get("SMTP_HOST").as_deref(), Some("smtp.platform"));
        assert_eq!(c.get("SMTP_PASSWORD").as_deref(), Some("platform-secret"), "its own From address still uses the shared relay");
        assert_eq!(c.get("DOCUSEAL_API_KEY"), None, "a company never uses the platform's own vendor accounts");
        assert_eq!(c.get("EMPTY"), None);
    }

    #[test]
    fn a_company_host_never_receives_the_platform_credentials() {
        let platform = platform();
        for key in ["SMTP_HOST", "SMTP_PORT", "SMTP_USERNAME", "SMTP_PASSWORD"] {
            let own = MapSource([(key.to_string(), "set-by-company".to_string())].into());
            let c = Layered { own: &own, fallback: &platform, shared_keys: SMTP, standalone_keys: &["SMTP_FROM"] };
            assert_eq!(c.get(key).as_deref(), Some("set-by-company"));
            for other in SMTP.iter().filter(|k| **k != key) {
                assert_eq!(c.get(other), None, "{key} set by the company: {other} must not come from the platform");
            }
        }
    }
}
