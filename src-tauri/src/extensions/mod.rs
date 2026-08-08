mod claude;
mod cli;
mod codex;
mod opencode;
pub mod refresh;

use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::{
    engines::EngineManager,
    models::{
        ExtensionActionResultDto, ExtensionCatalogKindRefreshDto, ExtensionItemDto,
        ExtensionProviderCapabilitiesDto, ExtensionSourceDto,
    },
};

const PROVIDERS: &[&str] = &["codex", "claude", "opencode"];
const KINDS: &[&str] = &["skill", "plugin", "mcp"];
const ACTIONS: &[&str] = &[
    "install",
    "uninstall",
    "enable",
    "disable",
    "remove",
    "authenticate",
    "logout",
];
const SCOPES: &[&str] = &["user", "project", "local"];
const MAX_CATALOG_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn provider_capabilities(provider_id: &str) -> ExtensionProviderCapabilitiesDto {
    match provider_id {
        "codex" => ExtensionProviderCapabilitiesDto {
            has_official_skill_catalog: false,
            can_toggle_skills: false,
            has_official_plugin_catalog: true,
            can_install_plugins: true,
            can_toggle_plugins: false,
            has_official_mcp_catalog: false,
            can_manage_mcp: true,
            can_authenticate_mcp: true,
        },
        "claude" => ExtensionProviderCapabilitiesDto {
            has_official_skill_catalog: false,
            can_toggle_skills: false,
            has_official_plugin_catalog: true,
            can_install_plugins: true,
            can_toggle_plugins: true,
            has_official_mcp_catalog: false,
            can_manage_mcp: true,
            can_authenticate_mcp: true,
        },
        "opencode" => ExtensionProviderCapabilitiesDto {
            has_official_skill_catalog: false,
            can_toggle_skills: false,
            has_official_plugin_catalog: false,
            can_install_plugins: false,
            can_toggle_plugins: false,
            has_official_mcp_catalog: false,
            can_manage_mcp: false,
            can_authenticate_mcp: true,
        },
        _ => unreachable!("provider must be validated before reading capabilities"),
    }
}

pub(crate) fn sources_from_items(items: &[ExtensionItemDto]) -> Vec<ExtensionSourceDto> {
    let sources = items
        .iter()
        .filter(|item| item.officially_available)
        .filter_map(|item| item.marketplace.as_deref().or(item.source.as_deref()))
        .collect::<BTreeSet<_>>();
    sources
        .into_iter()
        .map(|source| ExtensionSourceDto {
            id: source.to_string(),
            label: source.to_string(),
            official: true,
        })
        .collect()
}

fn read_catalog_manifest(path: &Path) -> Result<Value> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to inspect catalog manifest {}", path.display()))?;
    if metadata.len() > MAX_CATALOG_MANIFEST_BYTES {
        anyhow::bail!("catalog manifest is too large");
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read catalog manifest {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse catalog manifest {}", path.display()))
}

pub async fn refresh_catalog_kinds(
    manager: &EngineManager,
    provider_id: &str,
    cwd: Option<&str>,
    requested_kinds: &[String],
) -> Result<Vec<ExtensionCatalogKindRefreshDto>> {
    ensure_member("provider", provider_id, PROVIDERS)?;
    let requested = requested_kinds
        .iter()
        .map(|kind| kind.trim())
        .filter(|kind| !kind.is_empty())
        .collect::<BTreeSet<_>>();
    for kind in &requested {
        ensure_member("extension kind", kind, KINDS)?;
    }

    let mut results = Vec::new();
    for kind in KINDS {
        if !requested.contains(kind) {
            continue;
        }
        let result = match provider_id {
            "codex" => codex::refresh_kind(manager, cwd, kind).await,
            "claude" => claude::refresh_kind(manager, cwd, kind).await,
            "opencode" => opencode::refresh_kind(manager, cwd, kind).await,
            _ => unreachable!(),
        };
        results.push(result);
    }
    Ok(results)
}

pub async fn perform_action_for_item(
    provider_id: &str,
    item: ExtensionItemDto,
    action: &str,
    scope: Option<&str>,
    cwd: Option<&str>,
) -> Result<ExtensionActionResultDto> {
    ensure_member("provider", provider_id, PROVIDERS)?;
    ensure_member("extension kind", item.kind.as_str(), KINDS)?;
    ensure_member("action", action, ACTIONS)?;
    if let Some(scope) = scope {
        ensure_member("scope", scope, SCOPES)?;
    }
    validate_extension_id(&item.id)?;
    if item.provider_id != provider_id {
        anyhow::bail!("extension provider does not match action provider");
    }
    if !item
        .available_actions
        .iter()
        .any(|candidate| candidate == action)
    {
        anyhow::bail!("action is not available for this extension");
    }

    match provider_id {
        "codex" => codex::perform_action(&item, action, cwd).await,
        "claude" => claude::perform_action(&item, action, scope, cwd).await,
        "opencode" => opencode::perform_action(&item, action, cwd).await,
        _ => unreachable!(),
    }
}

fn ensure_member(label: &str, value: &str, allowed: &[&str]) -> Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        anyhow::bail!("unsupported {label}")
    }
}

fn validate_extension_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 1_024
        || value.starts_with('-')
        || value
            .chars()
            .any(|character| matches!(character, '\0' | '\n' | '\r'))
    {
        anyhow::bail!("invalid extension id");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_extension_id;

    #[test]
    fn rejects_option_and_line_injection_ids() {
        assert!(validate_extension_id("--help").is_err());
        assert!(validate_extension_id("valid\nother").is_err());
        assert!(validate_extension_id("normal@official").is_ok());
    }
}
