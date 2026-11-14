//! The settings and API keys a company can set from the dashboard, in both
//! single-company and platform mode. Only names listed here can be stored;
//! server keys (signing keys, database, OpenBao) are never in this list.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Field {
    pub name: &'static str,
    pub label: &'static str,
    pub secret: bool,
    pub placeholder: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Integration {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub fields: &'static [Field],
}

const fn f(name: &'static str, label: &'static str, secret: bool, placeholder: &'static str) -> Field {
    Field { name, label, secret, placeholder }
}

pub const CATALOG: &[Integration] = &[
    Integration {
        id: "email",
        name: "Email (SMTP)",
        description: "Sends interview invites, contract notices, and your notifications from your own address. Without it, Combox's shared sender is used.",
        fields: &[
            f("SMTP_HOST", "SMTP host", false, "smtp.example.com"),
            f("SMTP_PORT", "Port", false, "587"),
            f("SMTP_USERNAME", "Username", false, "hr@example.com"),
            f("SMTP_PASSWORD", "Password", true, ""),
            f("SMTP_FROM", "From address", false, "HR <hr@example.com>"),
        ],
    },
    Integration {
        id: "docuseal",
        name: "DocuSeal (contracts and tax forms)",
        description: "Sends your approved contract templates and collects W-9 / W-8BEN forms.",
        fields: &[
            f("DOCUSEAL_URL", "DocuSeal URL", false, "https://docuseal.example.com"),
            f("DOCUSEAL_API_KEY", "API key", true, ""),
            f("DOCUSEAL_TEMPLATES", "Approved templates (name=id,...)", false, "freelancer-services-v1=1,termination-notice-v1=2,tax-form-w9=3,tax-form-w8ben=4"),
        ],
    },
    Integration {
        id: "crm",
        name: "opendesk CRM (meeting notes)",
        description: "Where interview and meeting summaries are filed, as notes on the person's contact in opendesk. Create a workspace API key with the contacts:read and contacts:write scopes (Settings, API keys). Leave the URL empty for opendesk's hosted service. To let AI workers use your CRM, connect its MCP server on the Tools page instead.",
        fields: &[f("CRM_URL", "opendesk API URL (optional)", false, "https://api.opendesk.sh"), f("CRM_API_KEY", "API key", true, "od_live_...")],
    },
    Integration {
        id: "hiring",
        name: "Hiring marketplace",
        description: "Where human jobs are posted: your Combox Jobs board (default; hires paid in NEAR) or Upwork (hires paid through Upwork only).",
        fields: &[
            f("AXOM_MARKETPLACE", "Marketplace (board or upwork)", false, "board"),
            f("UPWORK_MCP_URL", "Upwork MCP URL", false, ""),
            f("UPWORK_MCP_TOKEN", "Upwork MCP token", true, ""),
        ],
    },
    Integration {
        id: "ai",
        name: "AI models",
        description: "Your Anthropic key lets your AI workers think: Claude reads each task, picks which of its allowed tools to use, and writes up the result (policy checks still apply to every tool call). The AI interviewer uses the same key and model, with Deepgram for speech-to-text and Cartesia for its voice. Anything unset here uses the interviewer's own settings.",
        fields: &[
            f("ANTHROPIC_API_KEY", "Anthropic API key", true, ""),
            f("AXOM_AI_MODEL", "Claude model (optional)", false, "claude-opus-5-5"),
            f("DEEPGRAM_API_KEY", "Deepgram API key (interviews)", true, ""),
            f("CARTESIA_API_KEY", "Cartesia API key (interviews)", true, ""),
        ],
    },
    Integration {
        id: "mcp",
        name: "MCP server access",
        description: "A bearer token (at least 32 characters) for AI clients using your meeting tools at /mcp.",
        fields: &[f("AXOM_MCP_TOKEN", "MCP token", true, "")],
    },
];

/// Settings the interview agent fetches for each meeting (never the rest):
/// the same Anthropic key and Claude model the AI workers use, plus speech.
pub const INTERVIEW_KEYS: &[&str] = &["DEEPGRAM_API_KEY", "CARTESIA_API_KEY", "ANTHROPIC_API_KEY", "AXOM_AI_MODEL"];

pub fn field(name: &str) -> Option<&'static Field> {
    CATALOG.iter().flat_map(|i| i.fields.iter()).find(|f| f.name == name)
}

/// Checks a value before it is stored; returns it trimmed.
pub fn validate<'a>(name: &str, value: &'a str) -> Result<(&'static Field, &'a str), String> {
    let f = field(name).ok_or_else(|| format!("{name} is not a supported setting"))?;
    let value = value.trim();
    if value.is_empty() || value.len() > 4000 || value.chars().any(|c| c.is_control()) {
        return Err("value: 1 to 4000 characters, single line".into());
    }
    if name == "AXOM_MCP_TOKEN" && value.len() < 32 {
        return Err("the MCP token must be at least 32 characters".into());
    }
    if name == "AXOM_AI_MODEL" && !(value.starts_with("claude-") && value.len() <= 64 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')) {
        return Err("model: a Claude model id such as claude-opus-5-5".into());
    }
    if name == "AXOM_MARKETPLACE" && !matches!(value, "board" | "upwork") {
        return Err("marketplace must be board or upwork".into());
    }
    Ok((f, value))
}

/// What is stored to recognise a value: the value itself for plain
/// settings, only the last four characters for secrets.
pub fn display(f: &Field, value: &str) -> String {
    if f.secret {
        hint(value)
    } else {
        value.chars().take(200).collect()
    }
}

pub fn hint(v: &str) -> String {
    let n = v.chars().count();
    if n <= 6 {
        "••••".into()
    } else {
        format!("••••{}", v.chars().skip(n - 4).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_never_reveal_short_values() {
        assert_eq!(hint("abc"), "••••");
        assert_eq!(hint("sk_live_1234567890"), "••••7890");
    }

    #[test]
    fn catalog_names_are_unique_and_valid() {
        let mut names: Vec<_> = CATALOG.iter().flat_map(|i| i.fields.iter().map(|f| f.name)).collect();
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), n);
        assert!(names.iter().all(|n| n.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')));
        assert!(INTERVIEW_KEYS.iter().all(|k| field(k).is_some()));
    }

    #[test]
    fn server_keys_are_never_settable() {
        for k in ["DATABASE_URL", "AXOM_SECRETS_KEY", "AXOM_APPROVAL_SIGNING_KEY", "AXOM_ATTESTATION_SIGNING_KEY", "OPENBAO_PROVISIONER_TOKEN", "OPENBAO_TOKEN"] {
            assert!(validate(k, "x").is_err());
        }
        assert!(validate("AXOM_MCP_TOKEN", "short").is_err());
        assert_eq!(validate("SMTP_HOST", "  smtp.example.com ").unwrap().1, "smtp.example.com");
    }
}
