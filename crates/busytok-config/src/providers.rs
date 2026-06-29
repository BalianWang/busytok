use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Protocol adapter kind. Determines how the sidecar communicates with the provider.
/// MVP supports only OpenAI-compatible providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenAiCompatible,
}

/// Non-sensitive provider metadata. Stored in settings.toml as `[[providers]]`.
/// API keys are stored separately in the OS keychain (see ProviderCredentialStore).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Immutable primary key. Used as keychain account name and profile binding target.
    pub id: String,
    /// Display name. Editable.
    pub name: String,
    /// Protocol adapter. Determines sidecar SDK mode.
    pub provider_kind: ProviderKind,
    /// OpenAI-compatible base URL (e.g. "https://api.deepseek.com/v1").
    pub base_url: String,
    /// Env var name the sidecar reads for the API key (e.g. "DEEPSEEK_API_KEY").
    /// Non-secret — only the name lives here; the value is in the keychain.
    pub api_key_env_name: String,
    /// Optional env var name for base URL override. Defaults to None.
    pub base_url_env_name: Option<String>,
    /// Whitelist of model IDs this provider supports. Validated before delegate.
    pub models: Vec<String>,
    /// Can be disabled without losing the keychain secret.
    pub enabled: bool,
}

// NOTE: No separate ProviderSettings wrapper needed. BusytokSettings
// already has `providers: Vec<ProviderConfig>` (added in lib.rs).
// Serde automatically serializes Vec<T> as [[providers]] array-of-tables.
// The array-of-tables test below uses BusytokSettings directly.

const KEYCHAIN_SERVICE: &str = "com.busytok.providers";

/// Abstraction over the OS keychain for storing provider API keys.
/// All operations use `service="com.busytok.providers"` and `account=provider_id`.
pub struct ProviderCredentialStore;

impl ProviderCredentialStore {
    /// Store (or overwrite) the API key for a provider.
    pub fn set_key(provider_id: &str, key: &str) -> Result<()> {
        tracing::debug!(provider_id, "storing provider API key in keychain");
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, provider_id)
            .context("failed to create keychain entry")?;
        entry
            .set_password(key)
            .context("failed to store API key in keychain")
    }

    /// Retrieve the API key for a provider. Returns Ok(None) if no key is stored.
    pub fn get_key(provider_id: &str) -> Result<Option<String>> {
        tracing::debug!(provider_id, "reading provider API key from keychain");
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, provider_id)
            .context("failed to create keychain entry")?;
        match entry.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("failed to read API key from keychain: {e}")),
        }
    }

    /// Delete the API key for a provider. Ok if no key existed.
    pub fn delete_key(provider_id: &str) -> Result<()> {
        tracing::debug!(provider_id, "deleting provider API key from keychain");
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, provider_id)
            .context("failed to create keychain entry")?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(anyhow::anyhow!(
                "failed to delete API key from keychain: {e}"
            )),
        }
    }

    /// Check whether a key is stored for a provider.
    pub fn has_key(provider_id: &str) -> bool {
        tracing::debug!(
            provider_id,
            "checking provider API key presence in keychain"
        );
        Self::get_key(provider_id)
            .map(|k| k.is_some())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BusytokSettings;

    #[test]
    fn provider_config_round_trips_toml() {
        let provider = ProviderConfig {
            id: "deepseek-prod".to_string(),
            name: "DeepSeek".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.deepseek.com/v1".to_string(),
            api_key_env_name: "DEEPSEEK_API_KEY".to_string(),
            base_url_env_name: Some("DEEPSEEK_BASE_URL".to_string()),
            models: vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()],
            enabled: true,
        };
        let toml_str = toml::to_string(&provider).unwrap();
        let parsed: ProviderConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.id, "deepseek-prod");
        assert_eq!(parsed.provider_kind, ProviderKind::OpenAiCompatible);
        assert_eq!(parsed.models.len(), 2);
        assert!(parsed.enabled);
        assert!(
            toml_str.contains("provider_kind = \"open_ai_compatible\""),
            "provider_kind must serialize as snake_case, got:\n{toml_str}"
        );
    }

    #[test]
    fn provider_config_array_serializes_as_array_of_tables() {
        let settings = BusytokSettings {
            providers: vec![ProviderConfig {
                id: "a".to_string(),
                name: "A".to_string(),
                provider_kind: ProviderKind::OpenAiCompatible,
                base_url: "https://a.example.com/v1".to_string(),
                api_key_env_name: "A_API_KEY".to_string(),
                base_url_env_name: None,
                models: vec![],
                enabled: true,
            }],
            ..Default::default()
        };
        let toml_str = toml::to_string(&settings).unwrap();
        assert!(
            toml_str.contains("[[providers]]"),
            "must serialize as array-of-tables, got:\n{toml_str}"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    #[ignore = "touches real macOS Keychain — run with --ignored"]
    fn provider_credential_store_round_trips_macos() {
        let store_id = "test-provider-roundtrip";
        let _ = ProviderCredentialStore::delete_key(store_id);
        assert!(!ProviderCredentialStore::has_key(store_id));
        ProviderCredentialStore::set_key(store_id, "sk-test-123").unwrap();
        assert!(ProviderCredentialStore::has_key(store_id));
        let key = ProviderCredentialStore::get_key(store_id).unwrap();
        assert_eq!(key.as_deref(), Some("sk-test-123"));
        ProviderCredentialStore::delete_key(store_id).unwrap();
        assert!(!ProviderCredentialStore::has_key(store_id));
    }
}
