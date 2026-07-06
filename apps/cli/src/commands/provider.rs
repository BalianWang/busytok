//! Handler for `busytok provider` — manage providers and their models.
//!
//! This module currently only dispatches a parsed `ProviderCommand` to its
//! handler stub. The actual RPC + rendering implementations land in Task 10.
use anyhow::Result;

use crate::ProviderCommand;

/// Dispatch a `ProviderCommand` to its handler.
pub async fn handle(cmd: ProviderCommand) -> Result<()> {
    match cmd {
        ProviderCommand::List { json } => handle_list(json).await,
        ProviderCommand::Add {
            base_url,
            api_key,
            kind,
            name,
            model_name,
            model_tags,
        } => handle_add(base_url, api_key, kind, name, model_name, model_tags).await,
        ProviderCommand::Show { id } => handle_show(id).await,
        ProviderCommand::Update {
            id,
            name,
            base_url,
            api_key,
            kind,
            enabled,
        } => handle_update(id, name, base_url, api_key, kind, enabled).await,
        ProviderCommand::Delete { id, yes } => handle_delete(id, yes).await,
        ProviderCommand::TestConnection { id } => handle_test_connection(id).await,
        ProviderCommand::Model { subcommand } => handle_model(subcommand).await,
    }
}

async fn handle_list(_json: bool) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_add(
    _base_url: String,
    _api_key: String,
    _kind: String,
    _name: Option<String>,
    _model_name: Option<String>,
    _model_tags: Option<String>,
) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_show(_id: String) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_update(
    _id: String,
    _name: Option<String>,
    _base_url: Option<String>,
    _api_key: Option<String>,
    _kind: Option<String>,
    _enabled: Option<bool>,
) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_delete(_id: String, _yes: bool) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_test_connection(_id: String) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_model(_subcommand: crate::ProviderModelCommand) -> Result<()> {
    anyhow::bail!("not yet implemented")
}
