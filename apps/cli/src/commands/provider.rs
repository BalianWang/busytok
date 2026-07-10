//! Handler for `busytok provider` — manage providers and their models.
use std::io::IsTerminal;

use anyhow::{Context, Result};
use busytok_domain::ProviderKind;
use busytok_protocol::dto::{
    ModelCreateRequestDto, ModelListRequestDto, ModelListResponseDto, ModelUpdateRequestDto,
    ProviderCreateRequestDto, ProviderDto, ProviderListResponseDto,
    ProviderTestConnectionResponseDto, ProviderUpdateRequestDto,
};
use busytok_protocol::{ControlRequest, ControlResponse};

use super::connect_client;
use crate::{ProviderCommand, ProviderModelCommand};

/// Dispatch a `ProviderCommand` to its handler.
pub async fn handle(cmd: ProviderCommand) -> Result<()> {
    match cmd {
        ProviderCommand::List { json } => handle_list(json).await,
        ProviderCommand::Add {
            url,
            key,
            kind,
            name,
            model,
            tags,
        } => handle_add(url, key, kind, name, model, tags).await,
        ProviderCommand::Show { id } => handle_show(id).await,
        ProviderCommand::Update {
            id,
            name,
            url,
            key,
            kind,
            enabled,
        } => handle_update(id, name, url, key, kind, enabled).await,
        ProviderCommand::Delete { id, yes } => handle_delete(id, yes).await,
        ProviderCommand::Test { id } => handle_test(id).await,
        ProviderCommand::Model { subcommand } => handle_model(subcommand).await,
    }
}

fn parse_tags(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn validate_base_url(input: &str) -> Result<()> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Base URL cannot be empty");
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        anyhow::bail!("URL must start with http:// or https://");
    }
    let host = extract_host(trimmed).unwrap_or("");
    if host.is_empty() {
        anyhow::bail!("URL must have a non-empty host");
    }
    Ok(())
}

fn extract_host(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let before_path = after_scheme.split('/').next()?;
    let before_colon = before_path.split(':').next()?;
    Some(before_colon)
}

fn derive_provider_name(url: &str, kind: &str) -> Option<String> {
    let host = extract_host(url)?;
    let parts: Vec<&str> = host.split('.').collect();
    let domain = parts
        .get(parts.len().saturating_sub(2))
        .copied()
        .unwrap_or(host);
    let kind_short = kind.replace("_compatible", "");
    Some(format!("{}_{}", domain, kind_short))
}

fn derive_unique_provider_name(
    url: &str,
    kind: &str,
    existing_names: &std::collections::HashSet<String>,
) -> String {
    let base = derive_provider_name(url, kind).unwrap_or_else(|| "provider".to_string());
    if !existing_names.contains(&base) {
        return base;
    }
    let mut i = 2;
    while existing_names.contains(&format!("{}_{}", base, i)) {
        i += 1;
    }
    format!("{}_{}", base, i)
}

/// Decision returned by `evaluate_delete_confirmation` — a pure enum so the
/// safety semantics are unit-testable without TTY/stdin/IO.
enum DeleteConfirmation {
    Proceed,
    Cancel,
    Bail,
}

/// Pure confirmation logic for destructive commands.
///
/// - `yes = true` → always Proceed (skip prompt)
/// - `yes = false` + non-TTY → Bail (refuse in non-interactive mode)
/// - `yes = false` + TTY + input "yes" → Proceed
/// - `yes = false` + TTY + other input → Cancel
fn evaluate_delete_confirmation(yes: bool, is_tty: bool, input: &str) -> DeleteConfirmation {
    if yes {
        return DeleteConfirmation::Proceed;
    }
    if !is_tty {
        return DeleteConfirmation::Bail;
    }
    if input.trim() == "yes" {
        DeleteConfirmation::Proceed
    } else {
        DeleteConfirmation::Cancel
    }
}

async fn handle_list(json: bool) -> Result<()> {
    let mut client = connect_client().await?;
    let response = client
        .call(ControlRequest::new("provider.list", serde_json::json!({})))
        .await?;
    match response {
        ControlResponse::Ok(value) => {
            let resp: ProviderListResponseDto = serde_json::from_value(value)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&resp.providers)?);
            } else {
                print_providers_table(&resp.providers);
            }
            Ok(())
        }
        ControlResponse::Err(err) => {
            anyhow::bail!("RPC error [{}]: {}", err.code, err.message)
        }
    }
}

fn print_providers_table(providers: &[ProviderDto]) {
    if providers.is_empty() {
        println!("No providers found.");
        return;
    }
    let w_id = 10;
    let w_name = providers
        .iter()
        .map(|p| p.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let w_kind = 22;
    let w_url = providers
        .iter()
        .map(|p| p.base_url.len())
        .max()
        .unwrap_or(8)
        .max(8);
    println!(
        "{:width_id$}  {:width_n$}  {:width_k$}  {:width_u$}  {:7}  {:5}",
        "ID",
        "NAME",
        "KIND",
        "BASE_URL",
        "ENABLED",
        "KEY",
        width_id = w_id,
        width_n = w_name,
        width_k = w_kind,
        width_u = w_url
    );
    for p in providers {
        // Char-safe truncation: byte-slicing `&p.id[..w_id]` panics if a
        // multi-byte UTF-8 char straddles the boundary. `chars().take(n)`
        // yields at most `n` scalar values, so shorter IDs are returned whole.
        let id_short: String = p.id.chars().take(w_id).collect();
        // `{:?}` on `ProviderKind::OpenAiCompatible` yields "OpenAiCompatible"
        // → "openaicompatible" (no underscore). Map to the wire string the GUI
        // and CLI flag parser both use.
        let kind_str = match p.provider_kind {
            ProviderKind::OpenAiCompatible => "openai_compatible",
            ProviderKind::AnthropicCompatible => "anthropic_compatible",
        };
        println!(
            "{:width_id$}  {:width_n$}  {:width_k$}  {:width_u$}  {:7}  {:5}",
            id_short,
            p.name,
            kind_str,
            p.base_url,
            if p.enabled { "yes" } else { "no" },
            if p.has_api_key { "yes" } else { "no" },
            width_id = w_id,
            width_n = w_name,
            width_k = w_kind,
            width_u = w_url
        );
    }
}

async fn handle_add(
    url: String,
    key: String,
    kind: String,
    name: Option<String>,
    model: Option<String>,
    tags: Option<String>,
) -> Result<()> {
    validate_base_url(&url)?;

    // Derive name (or use provided name). Collision-check against existing.
    let mut client = connect_client().await?;
    let list_resp = client
        .call(ControlRequest::new("provider.list", serde_json::json!({})))
        .await?;
    let existing_providers: ProviderListResponseDto = match list_resp {
        ControlResponse::Ok(v) => serde_json::from_value(v)?,
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    };
    let existing_names: std::collections::HashSet<String> = existing_providers
        .providers
        .iter()
        .map(|p| p.name.clone())
        .collect();

    let final_name = match name {
        Some(n) => n,
        None => derive_unique_provider_name(&url, &kind, &existing_names),
    };

    let parsed_kind = match kind.as_str() {
        "openai_compatible" => ProviderKind::OpenAiCompatible,
        "anthropic_compatible" => ProviderKind::AnthropicCompatible,
        other => anyhow::bail!("invalid kind: {other}"),
    };

    let create_req = ProviderCreateRequestDto {
        name: final_name.clone(),
        provider_kind: parsed_kind,
        base_url: url.clone(),
        api_key: Some(key),
        enabled: Some(true),
    };
    let provider: ProviderDto = {
        let resp = client
            .call(ControlRequest::new(
                "provider.create",
                serde_json::to_value(&create_req)?,
            ))
            .await?;
        match resp {
            ControlResponse::Ok(v) => serde_json::from_value(v)?,
            ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
        }
    };
    println!("Created provider: {} ({})", provider.name, provider.id);

    // Optional sync model creation
    if let Some(model_name) = model {
        let model_tags = parse_tags(tags.as_deref().unwrap_or(""));
        let model_req = ModelCreateRequestDto {
            provider_id: provider.id.clone(),
            model_id: model_name.clone(),
            enabled: Some(true),
            tags: model_tags,
            context_window: 200000,
            max_tokens: 8192,
            display_name: Some(model_name.clone()),
            reasoning: Some(true),
        };
        let resp = client
            .call(ControlRequest::new(
                "model.create",
                serde_json::to_value(&model_req)?,
            ))
            .await?;
        match resp {
            ControlResponse::Ok(_) => println!("Created model: {}", model_name),
            ControlResponse::Err(err) => anyhow::bail!(
                "Provider created, but model creation failed: RPC error [{}]: {}",
                err.code,
                err.message
            ),
        }
    }
    Ok(())
}

async fn handle_show(id: String) -> Result<()> {
    let mut client = connect_client().await?;
    let resp = client
        .call(ControlRequest::new("provider.list", serde_json::json!({})))
        .await?;
    let list: ProviderListResponseDto = match resp {
        ControlResponse::Ok(v) => serde_json::from_value(v)?,
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    };
    let provider = list
        .providers
        .into_iter()
        .find(|p| p.id == id)
        .with_context(|| format!("provider not found: {id}"))?;
    println!("{}", serde_json::to_string_pretty(&provider)?);
    Ok(())
}

async fn handle_update(
    id: String,
    name: Option<String>,
    url: Option<String>,
    key: Option<String>,
    kind: Option<String>,
    enabled: Option<bool>,
) -> Result<()> {
    if let Some(ref u) = url {
        validate_base_url(u)?;
    }
    let provider_kind = match kind.as_deref() {
        Some("openai_compatible") => Some(ProviderKind::OpenAiCompatible),
        Some("anthropic_compatible") => Some(ProviderKind::AnthropicCompatible),
        Some(other) => anyhow::bail!("invalid kind: {other}"),
        None => None,
    };
    let req = ProviderUpdateRequestDto {
        id: id.clone(),
        name,
        base_url: url,
        enabled,
        provider_kind,
        api_key: key.map(Some), // Some(Some(k)) = update; None = no change
    };
    let mut client = connect_client().await?;
    let resp = client
        .call(ControlRequest::new(
            "provider.update",
            serde_json::to_value(&req)?,
        ))
        .await?;
    match resp {
        ControlResponse::Ok(v) => {
            let updated: ProviderDto = serde_json::from_value(v)?;
            println!("Updated provider: {} ({})", updated.name, updated.id);
            Ok(())
        }
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }
}

async fn handle_delete(id: String, yes: bool) -> Result<()> {
    if !yes {
        let is_tty = std::io::stdin().is_terminal();
        let input = if is_tty {
            println!("Delete provider {} and all its models?", id);
            println!("Note: bound subagents will fail on next delegate. Rebind manually.");
            print!("Type 'yes' to confirm: ");
            use std::io::Write;
            std::io::stdout().flush()?;
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line
        } else {
            String::new()
        };
        match evaluate_delete_confirmation(yes, is_tty, &input) {
            DeleteConfirmation::Proceed => {}
            DeleteConfirmation::Cancel => {
                println!("Cancelled.");
                return Ok(());
            }
            DeleteConfirmation::Bail => {
                anyhow::bail!("Refusing to delete in non-interactive mode without --yes");
            }
        }
    }
    let mut client = connect_client().await?;
    let resp = client
        .call(ControlRequest::new(
            "provider.delete",
            serde_json::json!({ "id": id }),
        ))
        .await?;
    match resp {
        ControlResponse::Ok(_) => {
            println!("Deleted provider: {}", id);
            Ok(())
        }
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }
}

async fn handle_test(id: String) -> Result<()> {
    let mut client = connect_client().await?;
    let resp = client
        .call(ControlRequest::new(
            "provider.test_connection",
            serde_json::json!({ "id": id }),
        ))
        .await?;
    match resp {
        ControlResponse::Ok(v) => {
            let result: ProviderTestConnectionResponseDto = serde_json::from_value(v)?;
            if result.ok {
                println!("✓ connection ok");
                if let Some(models) = result.models_detected {
                    println!("  detected {} models", models.len());
                }
            } else {
                println!("✗ connection failed: {}", result.error.unwrap_or_default());
            }
            Ok(())
        }
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }
}

async fn handle_model(subcommand: ProviderModelCommand) -> Result<()> {
    match subcommand {
        ProviderModelCommand::List { provider_id, json } => {
            handle_model_list(provider_id, json).await
        }
        ProviderModelCommand::Add {
            provider_id,
            name,
            tags,
            context_window,
            max_tokens,
            reasoning,
            display_name,
        } => {
            handle_model_add(
                provider_id,
                name,
                tags,
                context_window,
                max_tokens,
                reasoning,
                display_name,
            )
            .await
        }
        ProviderModelCommand::Update {
            provider_id,
            model_id,
            tags,
            context_window,
            max_tokens,
            reasoning,
            enabled,
            display_name,
        } => {
            handle_model_update(
                provider_id,
                model_id,
                tags,
                context_window,
                max_tokens,
                reasoning,
                enabled,
                display_name,
            )
            .await
        }
        ProviderModelCommand::Delete {
            provider_id,
            model_id,
            yes,
        } => handle_model_delete(provider_id, model_id, yes).await,
    }
}

async fn handle_model_list(provider_id: String, json: bool) -> Result<()> {
    let req = ModelListRequestDto {
        provider_id: Some(provider_id),
        tags: vec![],
        include_disabled: true,
        sort: None,
        reasoning: None,
    };
    let mut client = connect_client().await?;
    let resp = client
        .call(ControlRequest::new(
            "model.list",
            serde_json::to_value(&req)?,
        ))
        .await?;
    match resp {
        ControlResponse::Ok(v) => {
            let list: ModelListResponseDto = serde_json::from_value(v)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list.models)?);
            } else {
                print_models_table(&list.models);
            }
            Ok(())
        }
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }
}

fn print_models_table(models: &[busytok_protocol::dto::ModelCatalogEntryDto]) {
    if models.is_empty() {
        println!("No models found.");
        return;
    }
    let w_id = models
        .iter()
        .map(|m| m.model_id.len())
        .max()
        .unwrap_or(5)
        .max(5);
    let w_tags = 20;
    println!(
        "{:width_m$}  {:6}  {:width_t$}",
        "MODEL",
        "ENABLE",
        "TAGS",
        width_m = w_id,
        width_t = w_tags
    );
    for m in models {
        let tags = m.tags.join(",");
        let en = if m.model_enabled { "yes" } else { "no" };
        println!(
            "{:width_m$}  {:6}  {:width_t$}",
            m.model_id,
            en,
            tags,
            width_m = w_id,
            width_t = w_tags
        );
    }
}

async fn handle_model_add(
    provider_id: String,
    name: String,
    tags: Option<String>,
    context_window: Option<i64>,
    max_tokens: Option<i64>,
    reasoning: bool,
    display_name: Option<String>,
) -> Result<()> {
    let model_tags = parse_tags(tags.as_deref().unwrap_or(""));
    let req = ModelCreateRequestDto {
        provider_id: provider_id.clone(),
        model_id: name.clone(),
        enabled: Some(true),
        tags: model_tags,
        context_window: context_window.unwrap_or(200000),
        max_tokens: max_tokens.unwrap_or(8192),
        display_name: Some(display_name.unwrap_or_else(|| name.clone())),
        reasoning: Some(reasoning),
    };
    let mut client = connect_client().await?;
    let resp = client
        .call(ControlRequest::new(
            "model.create",
            serde_json::to_value(&req)?,
        ))
        .await?;
    match resp {
        ControlResponse::Ok(_) => {
            println!("Created model: {} under provider: {}", name, provider_id);
            Ok(())
        }
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }
}

async fn handle_model_update(
    provider_id: String,
    model_id: String,
    tags: Option<String>,
    context_window: Option<i64>,
    max_tokens: Option<i64>,
    reasoning: Option<bool>,
    enabled: Option<bool>,
    display_name: Option<String>,
) -> Result<()> {
    // Resolve model_db_id via model.list (include_disabled: true)
    let list_req = ModelListRequestDto {
        provider_id: Some(provider_id.clone()),
        tags: vec![],
        include_disabled: true,
        sort: None,
        reasoning: None,
    };
    let mut client = connect_client().await?;
    let list_resp = client
        .call(ControlRequest::new(
            "model.list",
            serde_json::to_value(&list_req)?,
        ))
        .await?;
    let list: ModelListResponseDto = match list_resp {
        ControlResponse::Ok(v) => serde_json::from_value(v)?,
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    };
    let model = list
        .models
        .into_iter()
        .find(|m| m.model_id == model_id)
        .with_context(|| format!("model not found: {model_id} under provider {provider_id}"))?;

    let update_req = ModelUpdateRequestDto {
        id: model.model_db_id.clone(),
        enabled,
        display_name,
        reasoning,
        context_window,
        max_tokens,
    };
    let resp = client
        .call(ControlRequest::new(
            "model.update",
            serde_json::to_value(&update_req)?,
        ))
        .await?;
    match resp {
        ControlResponse::Ok(_) => {}
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }

    // Tags are updated via a separate RPC
    if let Some(tags_str) = tags {
        let parsed_tags = parse_tags(&tags_str);
        let tags_resp = client
            .call(ControlRequest::new(
                "model.tags.update",
                serde_json::json!({ "model_id": model.model_db_id, "tags": parsed_tags }),
            ))
            .await?;
        match tags_resp {
            ControlResponse::Ok(_) => {}
            ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
        }
    }
    println!("Updated model: {}", model_id);
    Ok(())
}

async fn handle_model_delete(provider_id: String, model_id: String, yes: bool) -> Result<()> {
    if !yes {
        let is_tty = std::io::stdin().is_terminal();
        let input = if is_tty {
            println!("Delete model {} under provider {}?", model_id, provider_id);
            println!("Note: bound subagents will fail on next delegate.");
            print!("Type 'yes' to confirm: ");
            use std::io::Write;
            std::io::stdout().flush()?;
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line
        } else {
            String::new()
        };
        match evaluate_delete_confirmation(yes, is_tty, &input) {
            DeleteConfirmation::Proceed => {}
            DeleteConfirmation::Cancel => {
                println!("Cancelled.");
                return Ok(());
            }
            DeleteConfirmation::Bail => {
                anyhow::bail!("Refusing to delete in non-interactive mode without --yes");
            }
        }
    }
    // Resolve model_db_id
    let list_req = ModelListRequestDto {
        provider_id: Some(provider_id.clone()),
        tags: vec![],
        include_disabled: true,
        sort: None,
        reasoning: None,
    };
    let mut client = connect_client().await?;
    let list_resp = client
        .call(ControlRequest::new(
            "model.list",
            serde_json::to_value(&list_req)?,
        ))
        .await?;
    let list: ModelListResponseDto = match list_resp {
        ControlResponse::Ok(v) => serde_json::from_value(v)?,
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    };
    let model = list
        .models
        .into_iter()
        .find(|m| m.model_id == model_id)
        .with_context(|| format!("model not found: {model_id} under provider {provider_id}"))?;

    let resp = client
        .call(ControlRequest::new(
            "model.delete",
            serde_json::json!({ "id": model.model_db_id }),
        ))
        .await?;
    match resp {
        ControlResponse::Ok(_) => {
            println!("Deleted model: {}", model_id);
            Ok(())
        }
        ControlResponse::Err(err) => anyhow::bail!("RPC error [{}]: {}", err.code, err.message),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
    use super::*;
    use async_trait::async_trait;
    use busytok_control::dispatch::RuntimeControl;
    use busytok_control::server::ControlServer;
    use busytok_control::TestRuntimeControl;
    use busytok_domain::ProviderKind;
    use busytok_protocol::dto::*;
    use serial_test::serial;
    use std::sync::Arc;

    // ─── Name derivation unit tests ──────────────────────────────────

    #[test]
    fn derive_provider_name_from_typical_url() {
        assert_eq!(
            derive_provider_name("https://api.deepseek.com/v1", "openai_compatible"),
            Some("deepseek_openai".to_string())
        );
    }

    #[test]
    fn derive_provider_name_strips_compatible_suffix() {
        assert_eq!(
            derive_provider_name("https://api.anthropic.com", "anthropic_compatible"),
            Some("anthropic_anthropic".to_string())
        );
    }

    #[test]
    fn derive_provider_name_falls_back_for_single_part_host() {
        assert_eq!(
            derive_provider_name("https://localhost:8080/v1", "openai_compatible"),
            Some("localhost_openai".to_string())
        );
    }

    #[test]
    fn derive_provider_name_handles_port() {
        // Mirrors the GUI test in providerFormUtils.test.ts: a single-part
        // host with a port falls back to the full host (port stripped).
        assert_eq!(
            derive_provider_name("http://host:3000", "openai_compatible"),
            Some("host_openai".to_string())
        );
    }

    #[test]
    fn derive_unique_provider_name_no_collision() {
        let existing: std::collections::HashSet<String> =
            ["other_openai".to_string()].into_iter().collect();
        assert_eq!(
            derive_unique_provider_name("https://api.deepseek.com", "openai_compatible", &existing),
            "deepseek_openai"
        );
    }

    #[test]
    fn derive_unique_provider_name_appends_2_on_collision() {
        let existing: std::collections::HashSet<String> =
            ["deepseek_openai".to_string()].into_iter().collect();
        assert_eq!(
            derive_unique_provider_name("https://api.deepseek.com", "openai_compatible", &existing),
            "deepseek_openai_2"
        );
    }

    #[test]
    fn derive_unique_provider_name_increments_until_unique() {
        let existing: std::collections::HashSet<String> = [
            "deepseek_openai".to_string(),
            "deepseek_openai_2".to_string(),
            "deepseek_openai_3".to_string(),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            derive_unique_provider_name("https://api.deepseek.com", "openai_compatible", &existing),
            "deepseek_openai_4"
        );
    }

    // ─── URL validation unit tests ───────────────────────────────────

    #[test]
    fn validate_base_url_accepts_https() {
        assert!(validate_base_url("https://api.deepseek.com/v1").is_ok());
    }

    #[test]
    fn validate_base_url_accepts_http() {
        assert!(validate_base_url("http://localhost:8080").is_ok());
    }

    #[test]
    fn validate_base_url_rejects_empty() {
        assert!(validate_base_url("").is_err());
    }

    #[test]
    fn validate_base_url_rejects_missing_protocol() {
        assert!(validate_base_url("api.deepseek.com").is_err());
    }

    #[test]
    fn validate_base_url_rejects_ftp() {
        assert!(validate_base_url("ftp://api.deepseek.com").is_err());
    }

    #[test]
    fn validate_base_url_rejects_empty_host_https() {
        assert!(validate_base_url("https://").is_err());
    }

    #[test]
    fn validate_base_url_rejects_empty_host_http() {
        assert!(validate_base_url("http:///foo").is_err());
    }

    // ─── Tag parsing ─────────────────────────────────────────────────

    #[test]
    fn parse_tags_empty_returns_empty_vec() {
        assert!(parse_tags("").is_empty());
    }

    #[test]
    fn parse_tags_splits_and_trims() {
        assert_eq!(
            parse_tags("cheap, fast , reasoning"),
            vec!["cheap", "fast", "reasoning"]
        );
    }

    #[test]
    fn parse_tags_drops_empty_entries() {
        assert_eq!(parse_tags("cheap,,fast,"), vec!["cheap", "fast"]);
    }

    // ─── Handler integration tests (against in-process ControlServer) ─
    //
    // `ProvidersRuntime` wraps `TestRuntimeControl` and returns a canned
    // `ProviderListResponseDto` from `provider_list`, delegating every other
    // method to the inner runtime. Following the established wrapper pattern
    // used by `ModelsRuntime` in `commands/models.rs`.

    struct ProvidersRuntime {
        inner: TestRuntimeControl,
        providers: Vec<ProviderDto>,
        models: Vec<ModelCatalogEntryDto>,
        test_result: ProviderTestConnectionResponseDto,
    }

    #[async_trait]
    impl RuntimeControl for ProvidersRuntime {
        async fn provider_list(&self) -> anyhow::Result<ProviderListResponseDto> {
            Ok(ProviderListResponseDto {
                providers: self.providers.clone(),
            })
        }
        // `handle_delete_proceeds_with_yes_flag` requires `provider_delete` to
        // succeed; the inner `TestRuntimeControl` bails with "not yet
        // implemented". Override to return Ok so the success-path test passes.
        async fn provider_delete(&self, _req: ProviderDeleteRequestDto) -> anyhow::Result<()> {
            Ok(())
        }
        // Echo the create request back as a ProviderDto so `handle_add` /
        // `handle_model_add` happy-path tests succeed without a SQL backend.
        async fn provider_create(
            &self,
            req: ProviderCreateRequestDto,
        ) -> anyhow::Result<ProviderDto> {
            Ok(ProviderDto {
                id: "prov-new".to_string(),
                name: req.name,
                provider_kind: req.provider_kind,
                base_url: req.base_url,
                enabled: req.enabled.unwrap_or(true),
                has_api_key: req.api_key.is_some(),
                created_at_ms: 0,
                updated_at_ms: 0,
            })
        }
        // Echo the update request back, applying patch fields over the first
        // seeded provider (or a default) so `handle_update` succeeds.
        async fn provider_update(
            &self,
            req: ProviderUpdateRequestDto,
        ) -> anyhow::Result<ProviderDto> {
            let base = self.providers.first().cloned().unwrap_or(ProviderDto {
                id: req.id.clone(),
                name: "updated".to_string(),
                provider_kind: ProviderKind::OpenAiCompatible,
                base_url: "https://api.example.com".to_string(),
                enabled: true,
                has_api_key: true,
                created_at_ms: 0,
                updated_at_ms: 0,
            });
            Ok(ProviderDto {
                id: req.id,
                name: req.name.unwrap_or(base.name),
                provider_kind: req.provider_kind.unwrap_or(base.provider_kind),
                base_url: req.base_url.unwrap_or(base.base_url),
                enabled: req.enabled.unwrap_or(base.enabled),
                has_api_key: base.has_api_key,
                created_at_ms: base.created_at_ms,
                updated_at_ms: 0,
            })
        }
        async fn provider_test_connection(
            &self,
            _req: ProviderTestConnectionRequestDto,
        ) -> anyhow::Result<ProviderTestConnectionResponseDto> {
            Ok(self.test_result.clone())
        }
        async fn model_list(
            &self,
            _req: ModelListRequestDto,
        ) -> anyhow::Result<ModelListResponseDto> {
            Ok(ModelListResponseDto {
                models: self.models.clone(),
            })
        }
        async fn model_create(
            &self,
            req: ModelCreateRequestDto,
        ) -> anyhow::Result<ModelCatalogEntryDto> {
            Ok(ModelCatalogEntryDto {
                provider_id: req.provider_id,
                provider_name: "test_provider".to_string(),
                provider_kind: ProviderKind::OpenAiCompatible,
                provider_enabled: true,
                model_db_id: "model-db-new".to_string(),
                model_id: req.model_id,
                model_enabled: req.enabled.unwrap_or(true),
                tags: req.tags,
                display_name: req.display_name,
                reasoning: req.reasoning.unwrap_or(false),
                context_window: Some(req.context_window),
                max_tokens: Some(req.max_tokens),
            })
        }
        async fn model_update(&self, _req: ModelUpdateRequestDto) -> anyhow::Result<()> {
            Ok(())
        }
        async fn model_delete(&self, _req: ModelDeleteRequestDto) -> anyhow::Result<()> {
            Ok(())
        }
        async fn model_tags_update(&self, _req: ModelTagUpdateDto) -> anyhow::Result<()> {
            Ok(())
        }
        // Everything else delegates to the inner runtime. The boilerplate is
        // verbatim from commands/models.rs:288-533 (only the struct name and
        // the overridden method change).
        async fn service_health(&self) -> anyhow::Result<ServiceHealthDto> {
            self.inner.service_health().await
        }
        async fn service_status(&self) -> anyhow::Result<ServiceStatusDto> {
            self.inner.service_status().await
        }
        async fn shell_status(&self) -> anyhow::Result<ShellStatusDto> {
            self.inner.shell_status().await
        }
        async fn overview_summary(
            &self,
            req: OverviewSummaryRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<OverviewSummaryDto>> {
            self.inner.overview_summary(req).await
        }
        async fn overview_trend(
            &self,
            req: OverviewTrendRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<OverviewTrendResponseDto>> {
            self.inner.overview_trend(req).await
        }
        async fn overview_heatmap(
            &self,
            req: OverviewHeatmapRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<OverviewHeatmapResponseDto>> {
            self.inner.overview_heatmap(req).await
        }
        async fn overview_rankings(
            &self,
            req: OverviewRankingsRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<OverviewRankingsResponseDto>> {
            self.inner.overview_rankings(req).await
        }
        async fn receipt_daily(
            &self,
            req: ReceiptDailyRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<ReceiptDailyDto>> {
            self.inner.receipt_daily(req).await
        }
        async fn activity_recent(
            &self,
            req: ActivityRecentRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<ActivityRecentResponseDto>> {
            self.inner.activity_recent(req).await
        }
        async fn activity_list(
            &self,
            req: ActivityListRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<ActivityListResponseDto>> {
            self.inner.activity_list(req).await
        }
        async fn activity_detail(
            &self,
            req: ActivityDetailRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<ActivityDetailDto>> {
            self.inner.activity_detail(req).await
        }
        async fn breakdown_list(
            &self,
            req: BreakdownListRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<BreakdownListResponseDto>> {
            self.inner.breakdown_list(req).await
        }
        async fn breakdown_detail(
            &self,
            req: BreakdownDetailRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<BreakdownDetailDto>> {
            self.inner.breakdown_detail(req).await
        }
        async fn clients_snapshot(
            &self,
            req: ClientsSnapshotRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<ClientsSnapshotDto>> {
            self.inner.clients_snapshot(req).await
        }
        async fn clients_detail(
            &self,
            req: ClientSourceDetailRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<ClientSourceDetailDto>> {
            self.inner.clients_detail(req).await
        }
        async fn settings_snapshot(&self) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
            self.inner.settings_snapshot().await
        }
        async fn settings_update(
            &self,
            req: SettingsUpdateRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
            self.inner.settings_update(req).await
        }
        async fn settings_diagnostics(
            &self,
        ) -> anyhow::Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
            self.inner.settings_diagnostics().await
        }
        async fn settings_recovery_action(
            &self,
            req: SettingsRecoveryActionRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>> {
            self.inner.settings_recovery_action(req).await
        }
        async fn live_window(
            &self,
            req: LiveWindowRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<LiveWindowDto>> {
            self.inner.live_window(req).await
        }
        async fn prompts_list(
            &self,
            req: PromptListQueryDto,
        ) -> anyhow::Result<ReadEnvelopeDto<PromptListResponseDto>> {
            self.inner.prompts_list(req).await
        }
        async fn prompts_get(
            &self,
            req: PromptGetRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_get(req).await
        }
        async fn prompts_create(
            &self,
            req: PromptCreateRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_create(req).await
        }
        async fn prompts_update(
            &self,
            req: PromptUpdateRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_update(req).await
        }
        async fn prompts_delete(
            &self,
            req: PromptDeleteRequestDto,
        ) -> anyhow::Result<PromptDeleteResultDto> {
            self.inner.prompts_delete(req).await
        }
        async fn prompts_use(
            &self,
            req: PromptUseRequestDto,
        ) -> anyhow::Result<PromptUseResultDto> {
            self.inner.prompts_use(req).await
        }
        async fn suggest_tags(
            &self,
            req: PromptSuggestTagsRequestDto,
        ) -> anyhow::Result<PromptSuggestTagsResponseDto> {
            self.inner.suggest_tags(req).await
        }
        async fn subagent_delegate(
            &self,
            req: SubagentDelegateRequestDto,
        ) -> anyhow::Result<SubagentDelegateResponseDto> {
            self.inner.subagent_delegate(req).await
        }
        async fn subagent_list(
            &self,
            req: SubagentListRequestDto,
        ) -> anyhow::Result<SubagentListResponseDto> {
            self.inner.subagent_list(req).await
        }
        async fn subagent_show(
            &self,
            req: SubagentResolveRequestDto,
        ) -> anyhow::Result<SubagentDetailDto> {
            self.inner.subagent_show(req).await
        }
        async fn subagent_tasks(
            &self,
            req: SubagentTasksRequestDto,
        ) -> anyhow::Result<SubagentTasksResponseDto> {
            self.inner.subagent_tasks(req).await
        }
        async fn subagent_hibernate(
            &self,
            req: SubagentResolveRequestDto,
        ) -> anyhow::Result<SubagentAckDto> {
            self.inner.subagent_hibernate(req).await
        }
        async fn subagent_delete(
            &self,
            req: SubagentDeleteRequestDto,
        ) -> anyhow::Result<SubagentAckDto> {
            self.inner.subagent_delete(req).await
        }
        async fn subagent_runtime_status(
            &self,
            req: SubagentRuntimeStatusRequestDto,
        ) -> anyhow::Result<ReadEnvelopeDto<SubagentRuntimeStatusDto>> {
            self.inner.subagent_runtime_status(req).await
        }
        async fn subagent_task_get(
            &self,
            req: SubagentTaskGetRequestDto,
        ) -> anyhow::Result<SubagentTaskDetailDto> {
            self.inner.subagent_task_get(req).await
        }
        async fn subagent_task_cancel(
            &self,
            req: busytok_protocol::dto::SubagentTaskCancelRequestDto,
        ) -> anyhow::Result<busytok_protocol::dto::SubagentTaskCancelResponseDto> {
            self.inner.subagent_task_cancel(req).await
        }
        async fn pi_sidecar_locator_update(
            &self,
            req: PiSidecarLocatorUpdateRequestDto,
        ) -> anyhow::Result<PiSidecarLocatorUpdateResponseDto> {
            self.inner.pi_sidecar_locator_update(req).await
        }
        async fn profile_create(&self, req: ProfileCreateRequestDto) -> anyhow::Result<ProfileDto> {
            self.inner.profile_create(req).await
        }
        async fn profile_update(&self, req: ProfileUpdateRequestDto) -> anyhow::Result<ProfileDto> {
            self.inner.profile_update(req).await
        }
        async fn profile_delete(&self, req: ProfileDeleteRequestDto) -> anyhow::Result<()> {
            self.inner.profile_delete(req).await
        }
        // `event_bus` is required by the trait (no default impl). Delegate to
        // the inner runtime's event bus.
        fn event_bus(&self) -> &busytok_events::AppEventBus {
            self.inner.event_bus()
        }
    }

    /// Hold a running `ControlServer` for the lifetime of the test.
    /// Verbatim from `commands/models.rs:224-247` — `spawn_for_test` only
    /// binds; the `run()` task must be spawned for connections to be accepted,
    /// and `shutdown()` must be called on drop.
    struct ServerHarness {
        server: Arc<ControlServer>,
        _task: tokio::task::JoinHandle<anyhow::Result<()>>,
    }

    async fn spawn_server(runtime: Arc<dyn RuntimeControl>) -> (ServerHarness, String) {
        let (server, socket_path) = ControlServer::spawn_for_test(runtime).await.unwrap();
        let server = Arc::new(server);
        let server_for_task = Arc::clone(&server);
        let task = tokio::spawn(async move { server_for_task.run().await });
        (
            ServerHarness {
                server,
                _task: task,
            },
            socket_path,
        )
    }

    impl Drop for ServerHarness {
        fn drop(&mut self) {
            self.server.shutdown();
        }
    }

    // Helper: spawn a test server with canned providers (and default empty
    // models + a successful test-connection response).
    async fn spawn_providers_server(providers: Vec<ProviderDto>) -> (ServerHarness, String) {
        spawn_providers_server_full(providers, vec![], default_test_result()).await
    }

    // Helper: spawn a test server with full control over canned providers,
    // models, and the `provider.test_connection` response.
    async fn spawn_providers_server_full(
        providers: Vec<ProviderDto>,
        models: Vec<ModelCatalogEntryDto>,
        test_result: ProviderTestConnectionResponseDto,
    ) -> (ServerHarness, String) {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(ProvidersRuntime {
            inner,
            providers,
            models,
            test_result,
        });
        spawn_server(runtime).await
    }

    fn default_test_result() -> ProviderTestConnectionResponseDto {
        ProviderTestConnectionResponseDto {
            ok: true,
            error: None,
            models_detected: Some(vec!["model-a".to_string(), "model-b".to_string()]),
        }
    }

    fn sample_provider() -> ProviderDto {
        ProviderDto {
            id: "prov-1".to_string(),
            name: "deepseek_openai".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.deepseek.com/v1".to_string(),
            enabled: true,
            has_api_key: true,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn sample_model_entry() -> ModelCatalogEntryDto {
        ModelCatalogEntryDto {
            provider_id: "prov-1".to_string(),
            provider_name: "deepseek_openai".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            provider_enabled: true,
            model_db_id: "model-db-1".to_string(),
            model_id: "deepseek-chat".to_string(),
            model_enabled: true,
            tags: vec!["cheap".to_string()],
            display_name: Some("DeepSeek Chat".to_string()),
            reasoning: false,
            context_window: Some(64000),
            max_tokens: Some(8192),
        }
    }

    #[tokio::test]
    #[serial]
    async fn handle_list_succeeds_with_providers() {
        let (harness, socket) = spawn_providers_server(vec![sample_provider()]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_list(false).await;
        drop(harness);
        assert!(result.is_ok(), "handle_list failed: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_list_json_succeeds() {
        let (harness, socket) = spawn_providers_server(vec![sample_provider()]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_list(true).await;
        drop(harness);
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[serial]
    async fn handle_list_empty_providers_succeeds() {
        let (harness, socket) = spawn_providers_server(vec![]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_list(false).await;
        drop(harness);
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[serial]
    async fn handle_delete_proceeds_with_yes_flag() {
        let (harness, socket) = spawn_providers_server(vec![sample_provider()]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_delete("prov-1".to_string(), true).await;
        drop(harness);
        assert!(
            result.is_ok(),
            "delete with --yes should proceed: {:?}",
            result
        );
    }

    // ── Pure confirmation logic tests (no TTY/stdin/IO needed) ──────────

    #[test]
    fn confirmation_proceeds_with_yes_flag() {
        // --yes always proceeds, regardless of TTY or input.
        assert!(matches!(
            evaluate_delete_confirmation(true, false, ""),
            DeleteConfirmation::Proceed
        ));
        assert!(matches!(
            evaluate_delete_confirmation(true, true, "no\n"),
            DeleteConfirmation::Proceed
        ));
    }

    #[test]
    fn confirmation_bails_in_non_tty_without_yes() {
        // Non-interactive mode without --yes must bail — this is the safety
        // guarantee that prevents accidental deletes in CI/scripts.
        assert!(matches!(
            evaluate_delete_confirmation(false, false, ""),
            DeleteConfirmation::Bail
        ));
    }

    #[test]
    fn confirmation_proceeds_in_tty_with_yes_input() {
        // TTY + user types "yes" → proceed.
        assert!(matches!(
            evaluate_delete_confirmation(false, true, "yes\n"),
            DeleteConfirmation::Proceed
        ));
    }

    #[test]
    fn confirmation_cancels_in_tty_with_non_yes_input() {
        // TTY + user types anything other than "yes" → cancel (no error).
        assert!(matches!(
            evaluate_delete_confirmation(false, true, "no\n"),
            DeleteConfirmation::Cancel
        ));
        assert!(matches!(
            evaluate_delete_confirmation(false, true, "\n"),
            DeleteConfirmation::Cancel
        ));
    }

    #[tokio::test]
    #[serial]
    async fn handle_show_succeeds_for_existing_provider() {
        let (harness, socket) = spawn_providers_server(vec![sample_provider()]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_show("prov-1".to_string()).await;
        drop(harness);
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[serial]
    async fn handle_show_fails_for_missing_provider() {
        let (harness, socket) = spawn_providers_server(vec![]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_show("nonexistent".to_string()).await;
        drop(harness);
        assert!(result.is_err());
    }

    // ── handle_add: provider create (+ optional model create) ──────────

    #[tokio::test]
    #[serial]
    async fn handle_add_succeeds_without_model() {
        let (harness, socket) = spawn_providers_server(vec![]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_add(
            "https://api.deepseek.com/v1".to_string(),
            "sk-test".to_string(),
            "openai_compatible".to_string(),
            None,
            None,
            None,
        )
        .await;
        drop(harness);
        assert!(result.is_ok(), "handle_add failed: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_add_succeeds_with_model_and_tags() {
        // Exercises both provider.create and model.create paths.
        let (harness, socket) = spawn_providers_server(vec![]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_add(
            "https://api.deepseek.com/v1".to_string(),
            "sk-test".to_string(),
            "openai_compatible".to_string(),
            None,
            Some("deepseek-chat".to_string()),
            Some("cheap,fast".to_string()),
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_add with model failed: {:?}",
            result.err()
        );
    }

    // ── handle_update: provider update ─────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn handle_update_succeeds_with_name_change() {
        let (harness, socket) = spawn_providers_server(vec![sample_provider()]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_update(
            "prov-1".to_string(),
            Some("renamed_provider".to_string()),
            None,
            None,
            None,
            None,
        )
        .await;
        drop(harness);
        assert!(result.is_ok(), "handle_update failed: {:?}", result.err());
    }

    // ── handle_test: provider test_connection ──────────────────────────

    #[tokio::test]
    #[serial]
    async fn handle_test_succeeds() {
        // `default_test_result()` returns ok:true with 2 detected models,
        // exercising the success-rendering branch of handle_test.
        let (harness, socket) = spawn_providers_server(vec![sample_provider()]).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_test("prov-1".to_string()).await;
        drop(harness);
        assert!(result.is_ok(), "handle_test failed: {:?}", result.err());
    }

    // ── handle_model_list: model list ──────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn handle_model_list_table_succeeds() {
        let (harness, socket) = spawn_providers_server_full(
            vec![sample_provider()],
            vec![sample_model_entry()],
            default_test_result(),
        )
        .await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_model_list("prov-1".to_string(), false).await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_model_list table failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_model_list_json_succeeds() {
        let (harness, socket) = spawn_providers_server_full(
            vec![sample_provider()],
            vec![sample_model_entry()],
            default_test_result(),
        )
        .await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_model_list("prov-1".to_string(), true).await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_model_list json failed: {:?}",
            result.err()
        );
    }

    // ── handle_model_add: model create ─────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn handle_model_add_succeeds() {
        let (harness, socket) =
            spawn_providers_server_full(vec![sample_provider()], vec![], default_test_result())
                .await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_model_add(
            "prov-1".to_string(),
            "deepseek-chat".to_string(),
            Some("cheap,fast".to_string()),
            None,
            None,
            false,
            None,
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_model_add failed: {:?}",
            result.err()
        );
    }

    // ── handle_model_update: model update + optional tags update ───────

    #[tokio::test]
    #[serial]
    async fn handle_model_update_succeeds_with_tags_change() {
        // Passes a tags change so both model.update AND model.tags.update are
        // exercised. Both overrides must return Ok for the handler to succeed.
        let (harness, socket) = spawn_providers_server_full(
            vec![sample_provider()],
            vec![sample_model_entry()],
            default_test_result(),
        )
        .await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_model_update(
            "prov-1".to_string(),
            "deepseek-chat".to_string(),
            Some("cheap,fast,reasoning".to_string()),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_model_update with tags failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_model_update_fails_for_missing_model() {
        // model.list returns empty → model_id lookup bails with "model not
        // found". Mirrors handle_show_fails_for_missing_provider.
        let (harness, socket) =
            spawn_providers_server_full(vec![sample_provider()], vec![], default_test_result())
                .await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_model_update(
            "prov-1".to_string(),
            "nonexistent-model".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        drop(harness);
        assert!(
            result.is_err(),
            "handle_model_update should fail for missing model"
        );
    }

    // ── handle_model_delete: model delete with --yes ───────────────────

    #[tokio::test]
    #[serial]
    async fn handle_model_delete_succeeds_with_yes_flag() {
        let (harness, socket) = spawn_providers_server_full(
            vec![sample_provider()],
            vec![sample_model_entry()],
            default_test_result(),
        )
        .await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result =
            handle_model_delete("prov-1".to_string(), "deepseek-chat".to_string(), true).await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_model_delete with --yes failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_model_delete_fails_for_missing_model() {
        let (harness, socket) =
            spawn_providers_server_full(vec![sample_provider()], vec![], default_test_result())
                .await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result =
            handle_model_delete("prov-1".to_string(), "nonexistent-model".to_string(), true).await;
        drop(harness);
        assert!(
            result.is_err(),
            "handle_model_delete should fail for missing model"
        );
    }
}
