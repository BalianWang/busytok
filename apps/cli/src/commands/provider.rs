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

async fn handle_list(_json: bool) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_add(
    _url: String,
    _key: String,
    _kind: String,
    _name: Option<String>,
    _model: Option<String>,
    _tags: Option<String>,
) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_show(_id: String) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_update(
    _id: String,
    _name: Option<String>,
    _url: Option<String>,
    _key: Option<String>,
    _kind: Option<String>,
    _enabled: Option<bool>,
) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_delete(_id: String, _yes: bool) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_test(_id: String) -> Result<()> {
    anyhow::bail!("not yet implemented")
}

async fn handle_model(_subcommand: crate::ProviderModelCommand) -> Result<()> {
    anyhow::bail!("not yet implemented")
}
