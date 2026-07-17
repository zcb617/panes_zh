use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

use anyhow::Result;
use serde_json::Value;

use crate::{
    engines::EngineManager,
    models::{
        CodexProtocolDiagnosticsDto, ExtensionActionResultDto, ExtensionCatalogKindRefreshDto,
        ExtensionItemDto,
    },
};

use super::cli;

const PROVIDER_ID: &str = "codex";
const OFFICIAL_MARKETPLACES: &[&str] = &["openai-bundled", "openai-curated"];

pub async fn refresh_kind(
    manager: &EngineManager,
    cwd: Option<&str>,
    kind: &str,
) -> ExtensionCatalogKindRefreshDto {
    match kind {
        "skill" => {
            let result = match cwd {
                Some(cwd) => manager.list_codex_skills(cwd).await,
                None => Ok(Vec::new()),
            };
            match result {
                Ok(skills) => ExtensionCatalogKindRefreshDto::success(
                    kind,
                    skills
                        .into_iter()
                        .map(|skill| ExtensionItemDto {
                            id: skill.path.clone(),
                            provider_id: PROVIDER_ID.to_string(),
                            kind: "skill".to_string(),
                            name: skill.name,
                            description: non_empty(skill.description),
                            version: None,
                            scope: normalize_skill_scope(&skill.scope),
                            source: non_empty(skill.scope),
                            marketplace: None,
                            path: Some(skill.path),
                            parent_plugin_id: None,
                            category: None,
                            officially_available: false,
                            catalog_authority: None,
                            installed: Some(true),
                            configured: None,
                            enabled: Some(skill.enabled),
                            health: if skill.enabled { "healthy" } else { "unknown" }.to_string(),
                            auth_state: None,
                            available_actions: Vec::new(),
                            requires_new_session: false,
                            read_only_reason: Some("codex_skill_toggle".to_string()),
                            warning: None,
                        })
                        .collect(),
                ),
                Err(_) => ExtensionCatalogKindRefreshDto::failure(kind),
            }
        }
        "plugin" => {
            let plugin_args = vec![
                "plugin".to_string(),
                "list".to_string(),
                "--available".to_string(),
                "--json".to_string(),
            ];
            let (plugins_result, health_result) = tokio::join!(
                cli::run_json("codex", &plugin_args, cwd),
                manager.health(PROVIDER_ID),
            );
            match plugins_result {
                Ok(value) => {
                    let categories = HashMap::new();
                    let diagnostics = health_result
                        .ok()
                        .and_then(|health| health.protocol_diagnostics);
                    ExtensionCatalogKindRefreshDto::success(
                        kind,
                        parse_plugins(&value, diagnostics.as_ref(), &categories),
                    )
                }
                Err(_) => ExtensionCatalogKindRefreshDto::failure(kind),
            }
        }
        "mcp" => {
            let mcp_args = vec!["mcp".to_string(), "list".to_string(), "--json".to_string()];
            let (mcp_result, health_result) = tokio::join!(
                cli::run_json("codex", &mcp_args, cwd),
                manager.health(PROVIDER_ID),
            );
            match mcp_result {
                Ok(value) => {
                    let diagnostics = health_result
                        .ok()
                        .and_then(|health| health.protocol_diagnostics);
                    ExtensionCatalogKindRefreshDto::success(
                        kind,
                        parse_mcp_servers(&value, diagnostics.as_ref()),
                    )
                }
                Err(_) => ExtensionCatalogKindRefreshDto::failure(kind),
            }
        }
        _ => ExtensionCatalogKindRefreshDto::failure(kind),
    }
}

pub async fn perform_action(
    item: &ExtensionItemDto,
    action: &str,
    cwd: Option<&str>,
) -> Result<ExtensionActionResultDto> {
    let args = match (item.kind.as_str(), action) {
        ("plugin", "install") => vec![
            "plugin".to_string(),
            "add".to_string(),
            item.id.clone(),
            "--json".to_string(),
        ],
        ("plugin", "uninstall") => vec![
            "plugin".to_string(),
            "remove".to_string(),
            item.id.clone(),
            "--json".to_string(),
        ],
        ("mcp", "remove") => vec!["mcp".to_string(), "remove".to_string(), item.id.clone()],
        ("mcp", "authenticate") => {
            vec!["mcp".to_string(), "login".to_string(), item.id.clone()]
        }
        ("mcp", "logout") => vec!["mcp".to_string(), "logout".to_string(), item.id.clone()],
        _ => anyhow::bail!("unsupported Codex extension action"),
    };

    cli::run_action("codex", &args, cwd).await?;
    Ok(ExtensionActionResultDto {
        provider_id: PROVIDER_ID.to_string(),
        kind: item.kind.clone(),
        extension_id: item.id.clone(),
        action: action.to_string(),
        requires_new_session: item.requires_new_session,
    })
}

fn parse_plugins(
    value: &Value,
    diagnostics: Option<&CodexProtocolDiagnosticsDto>,
    categories: &HashMap<String, String>,
) -> Vec<ExtensionItemDto> {
    let mut descriptions = HashMap::<String, String>::new();
    if let Some(diagnostics) = diagnostics {
        for marketplace in &diagnostics.plugin_marketplaces {
            for plugin in &marketplace.plugins {
                let id = plugin_selector(&plugin.id, &marketplace.name);
                if let Some(description) = plugin.description.clone().or_else(|| {
                    (!plugin.capabilities.is_empty()).then(|| plugin.capabilities.join(", "))
                }) {
                    descriptions.insert(id, description);
                }
            }
        }
    }

    let mut items = BTreeMap::<String, ExtensionItemDto>::new();
    for plugin in value
        .get("installed")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = string_field(plugin, "pluginId") else {
            continue;
        };
        let marketplace = string_field(plugin, "marketplaceName");
        let item = ExtensionItemDto {
            id: id.clone(),
            provider_id: PROVIDER_ID.to_string(),
            kind: "plugin".to_string(),
            name: string_field(plugin, "name").unwrap_or_else(|| plugin_name(&id)),
            description: descriptions.get(&id).cloned(),
            version: string_field(plugin, "version"),
            scope: "user".to_string(),
            source: marketplace.clone(),
            marketplace,
            path: nested_string_field(plugin, &["source", "path"]),
            parent_plugin_id: None,
            category: categories
                .get(&id)
                .cloned()
                .or_else(|| category_from_plugin_source(plugin)),
            officially_available: false,
            catalog_authority: None,
            installed: Some(true),
            configured: None,
            enabled: Some(bool_field(plugin, "enabled").unwrap_or(true)),
            health: "healthy".to_string(),
            auth_state: None,
            available_actions: vec!["uninstall".to_string()],
            requires_new_session: true,
            read_only_reason: None,
            warning: None,
        };
        items.insert(id, item);
    }

    for plugin in value
        .get("available")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = string_field(plugin, "pluginId") else {
            continue;
        };
        let Some(marketplace) = string_field(plugin, "marketplaceName") else {
            continue;
        };
        if !is_official_marketplace(&marketplace) {
            continue;
        }

        if let Some(installed) = items.get_mut(&id) {
            installed.officially_available = true;
            installed.catalog_authority = Some("provider_official".to_string());
            installed.category = installed
                .category
                .take()
                .or_else(|| categories.get(&id).cloned())
                .or_else(|| category_from_plugin_source(plugin));
            installed.description = installed
                .description
                .take()
                .or_else(|| string_field(plugin, "description"));
            continue;
        }

        items.insert(
            id.clone(),
            ExtensionItemDto {
                id: id.clone(),
                provider_id: PROVIDER_ID.to_string(),
                kind: "plugin".to_string(),
                name: string_field(plugin, "name").unwrap_or_else(|| plugin_name(&id)),
                description: string_field(plugin, "description")
                    .or_else(|| descriptions.get(&id).cloned()),
                version: string_field(plugin, "version"),
                scope: "user".to_string(),
                source: Some(marketplace.clone()),
                marketplace: Some(marketplace),
                path: None,
                parent_plugin_id: None,
                category: categories
                    .get(&id)
                    .cloned()
                    .or_else(|| category_from_plugin_source(plugin)),
                officially_available: true,
                catalog_authority: Some("provider_official".to_string()),
                installed: Some(false),
                configured: None,
                enabled: Some(false),
                health: "unknown".to_string(),
                auth_state: None,
                available_actions: vec!["install".to_string()],
                requires_new_session: true,
                read_only_reason: None,
                warning: None,
            },
        );
    }

    items.into_values().collect()
}

fn parse_mcp_servers(
    value: &Value,
    diagnostics: Option<&CodexProtocolDiagnosticsDto>,
) -> Vec<ExtensionItemDto> {
    let diagnostic_by_name = diagnostics
        .map(|diagnostics| {
            diagnostics
                .mcp_servers
                .iter()
                .map(|server| (server.name.as_str(), server))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|server| {
            let name = string_field(server, "name")?;
            let enabled = bool_field(server, "enabled").unwrap_or(true);
            let auth_status = string_field(server, "auth_status")
                .or_else(|| string_field(server, "authStatus"))
                .unwrap_or_else(|| "unknown".to_string());
            let auth_state = normalize_auth_state(&auth_status);
            let parent_plugin_id = managed_plugin_name(&name);
            let managed_by_plugin = parent_plugin_id.is_some();
            let mut available_actions = Vec::new();
            if !managed_by_plugin {
                available_actions.push("remove".to_string());
                match auth_state.as_deref() {
                    Some("required") | Some("failed") => {
                        available_actions.push("authenticate".to_string())
                    }
                    Some("authenticated") => available_actions.push("logout".to_string()),
                    _ => {}
                }
            }

            let diagnostic = diagnostic_by_name.get(name.as_str()).copied();
            Some(ExtensionItemDto {
                id: name.clone(),
                provider_id: PROVIDER_ID.to_string(),
                kind: "mcp".to_string(),
                name,
                description: diagnostic.map(|server| {
                    format!(
                        "{} tools · {} resources",
                        server.tool_count, server.resource_count
                    )
                }),
                version: None,
                scope: "user".to_string(),
                source: transport_label(server.get("transport")),
                marketplace: None,
                path: None,
                parent_plugin_id,
                category: None,
                officially_available: false,
                catalog_authority: None,
                installed: None,
                configured: Some(true),
                enabled: Some(enabled),
                health: if !enabled {
                    "disconnected"
                } else if auth_state.as_deref() == Some("required") {
                    "auth_required"
                } else if diagnostic.is_some() {
                    "healthy"
                } else {
                    "unknown"
                }
                .to_string(),
                auth_state,
                available_actions,
                requires_new_session: false,
                read_only_reason: managed_by_plugin.then(|| "plugin_managed_mcp".to_string()),
                // MCP diagnostic text can include transport details. Persist only the
                // normalized health state, never a raw configuration-derived reason.
                warning: None,
            })
        })
        .collect()
}

fn plugin_selector(id: &str, marketplace: &str) -> String {
    if id.contains('@') {
        id.to_string()
    } else {
        format!("{id}@{marketplace}")
    }
}

fn plugin_name(id: &str) -> String {
    id.split('@').next().unwrap_or(id).to_string()
}

fn managed_plugin_name(name: &str) -> Option<String> {
    let plugin_name = name.strip_prefix("plugin:")?.split(':').next()?.trim();
    (!plugin_name.is_empty()).then(|| plugin_name.to_string())
}

fn category_from_plugin_source(plugin: &Value) -> Option<String> {
    let source_path = nested_string_field(plugin, &["source", "path"])?;
    let manifest_path = Path::new(&source_path).join(".codex-plugin/plugin.json");
    let manifest = super::read_catalog_manifest(&manifest_path).ok()?;
    nested_string_field(&manifest, &["interface", "category"])
}

fn is_official_marketplace(value: &str) -> bool {
    OFFICIAL_MARKETPLACES.contains(&value)
}

fn normalize_skill_scope(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "system" | "builtin" => "builtin",
        "workspace" | "repo" | "project" => "project",
        "plugin" => "plugin",
        "managed" | "enterprise" => "managed",
        _ => "user",
    }
    .to_string()
}

fn normalize_auth_state(value: &str) -> Option<String> {
    let normalized = value.to_ascii_lowercase();
    if normalized.contains("authenticated") || normalized.contains("logged") {
        Some("authenticated".to_string())
    } else if normalized.contains("required") || normalized.contains("unauth") {
        Some("required".to_string())
    } else if normalized.contains("fail") || normalized.contains("error") {
        Some("failed".to_string())
    } else {
        Some("unknown".to_string())
    }
}

fn transport_label(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) => {
            let normalized = value.trim().to_ascii_lowercase();
            if normalized.contains("stdio") {
                Some("stdio".to_string())
            } else if normalized.contains("http") || normalized.contains("sse") {
                Some("http".to_string())
            } else {
                Some("MCP".to_string())
            }
        }
        Some(Value::Object(value)) => ["type", "kind", "transport"]
            .iter()
            .find_map(|key| value.get(*key).and_then(Value::as_str))
            .map(str::to_string)
            .or_else(|| {
                if value.contains_key("command") {
                    Some("stdio".to_string())
                } else if value.contains_key("url") {
                    Some("http".to_string())
                } else {
                    Some("MCP".to_string())
                }
            }),
        _ => Some("MCP".to_string()),
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn bool_field(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

fn nested_string_field(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_str().map(str::to_string)
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::parse_plugins;

    #[test]
    fn official_available_and_all_installed_plugins_are_preserved() {
        let value = json!({
            "installed": [
                {
                    "pluginId": "local-tool@personal",
                    "name": "local-tool",
                    "marketplaceName": "personal",
                    "installed": true,
                    "enabled": true
                }
            ],
            "available": [
                {
                    "pluginId": "official-tool@openai-curated",
                    "name": "official-tool",
                    "marketplaceName": "openai-curated",
                    "installed": false,
                    "enabled": false
                },
                {
                    "pluginId": "community-tool@community",
                    "name": "community-tool",
                    "marketplaceName": "community",
                    "installed": false,
                    "enabled": false
                }
            ]
        });

        let items = parse_plugins(&value, None, &HashMap::new());
        assert!(items.iter().any(|item| item.id == "local-tool@personal"));
        assert!(items
            .iter()
            .any(|item| item.id == "official-tool@openai-curated" && item.officially_available));
        assert!(!items
            .iter()
            .any(|item| item.id == "community-tool@community"));
    }
}
