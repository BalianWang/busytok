//! Provider / Model / Tag domain model for the SQL-backed catalog.
use serde::{Deserialize, Serialize};

/// Provider kind. MVP only supports OpenAI-compatible.
/// Kept in domain (not config) because it is wire-level vocabulary
/// shared by protocol, store, and runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
}

/// Provider — connection config + credential. Stored in SQL `providers` table.
/// `api_key` is the plaintext key; DTOs never expose it (use `ProviderSummary`).
#[derive(Debug, Clone)]
pub struct Provider {
    pub id: String,  // UUID v4, 系统生成（store 层生成，不由用户提供）
    pub name: String,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub enabled: bool,
    pub api_key: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Provider without the secret — safe for list views and DTOs.
#[derive(Debug, Clone)]
pub struct ProviderSummary {
    pub id: String,
    pub name: String,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub enabled: bool,
    pub has_api_key: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl From<&Provider> for ProviderSummary {
    fn from(p: &Provider) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            provider_kind: p.provider_kind.clone(),
            base_url: p.base_url.clone(),
            enabled: p.enabled,
            has_api_key: p.api_key.is_some(),
            created_at_ms: p.created_at_ms,
            updated_at_ms: p.updated_at_ms,
        }
    }
}

/// Model — a routable model instance under a provider.
/// `model_id` is immutable after creation (no rename).
#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,           // DB primary key (UUID)
    pub provider_id: String,  // FK -> providers.id
    pub model_id: String,     // immutable, e.g. "gpt-4o"
    pub enabled: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Tag row — many-to-many between models and tag strings.
#[derive(Debug, Clone)]
pub struct ModelTag {
    pub model_id: String,  // FK -> models.id
    pub tag: String,
}

/// Unified model catalog entry — joined view for CLI/GUI/routing.
#[derive(Debug, Clone)]
pub struct ModelCatalogEntry {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_kind: ProviderKind,
    pub provider_enabled: bool,
    pub model_db_id: String,
    pub model_id: String,
    pub model_enabled: bool,
    pub tags: Vec<String>,
}

/// Filter for `list_models_filtered`.
/// `include_disabled=false` filters BOTH provider_enabled and model_enabled.
#[derive(Debug, Clone, Default)]
pub struct ModelCatalogFilter {
    pub provider_id: Option<String>,
    pub tags: Vec<String>,      // AND semantics
    pub include_disabled: bool,
}

/// Profile → model reference snapshot. Collected from settings.toml profiles
/// and passed to store-layer reference-check functions. Keeps store layer
/// decoupled from config layer.
#[derive(Debug, Clone)]
pub struct ProfileModelRef {
    pub provider_id: String,
    pub model_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_serde_openai_compatible() {
        let json = serde_json::to_string(&ProviderKind::OpenAiCompatible).unwrap();
        assert_eq!(json, "\"openai_compatible\"");
        let parsed: ProviderKind = serde_json::from_str("\"openai_compatible\"").unwrap();
        assert_eq!(parsed, ProviderKind::OpenAiCompatible);
    }

    #[test]
    fn provider_summary_has_api_key_false_when_api_key_none() {
        let p = Provider {
            id: "p1".into(),
            name: "Test".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: None,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };
        let s = ProviderSummary::from(&p);
        assert!(!s.has_api_key);
    }
}
