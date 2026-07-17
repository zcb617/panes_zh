use tauri::{AppHandle, State};

use crate::{
    extensions,
    models::{CachedExtensionCatalogDto, ExtensionActionResultDto, ExtensionItemDto},
    state::AppState,
};

#[tauri::command]
pub async fn get_extension_catalog(
    state: State<'_, AppState>,
    provider_id: String,
    cwd: Option<String>,
) -> Result<CachedExtensionCatalogDto, String> {
    let cwd = normalize_cwd(cwd);
    extensions::refresh::load_cached_catalog(&state, provider_id.trim(), cwd.as_deref())
        .await
        .map_err(err_to_string)
}

#[tauri::command]
pub async fn request_extension_catalog_refresh(
    app: AppHandle,
    state: State<'_, AppState>,
    provider_id: String,
    cwd: Option<String>,
    kinds: Option<Vec<String>>,
) -> Result<CachedExtensionCatalogDto, String> {
    let cwd = normalize_cwd(cwd);
    extensions::refresh::request_catalog_refresh(
        app,
        state.inner().clone(),
        provider_id.trim(),
        cwd.clone(),
        kinds.unwrap_or_default(),
    )
    .await
    .map_err(err_to_string)?;
    extensions::refresh::load_cached_catalog(&state, provider_id.trim(), cwd.as_deref())
        .await
        .map_err(err_to_string)
}

#[tauri::command]
pub async fn get_extension_details(
    state: State<'_, AppState>,
    provider_id: String,
    kind: String,
    extension_id: String,
    cwd: Option<String>,
) -> Result<ExtensionItemDto, String> {
    let cwd = normalize_cwd(cwd);
    let catalog =
        extensions::refresh::load_cached_catalog(&state, provider_id.trim(), cwd.as_deref())
            .await
            .map_err(err_to_string)?;

    catalog
        .items
        .into_iter()
        .find(|item| item.kind == kind && item.id == extension_id)
        .ok_or_else(|| "extension not found in the current catalog".to_string())
}

#[tauri::command]
pub async fn perform_extension_action(
    app: AppHandle,
    state: State<'_, AppState>,
    provider_id: String,
    kind: String,
    extension_id: String,
    action: String,
    scope: Option<String>,
    cwd: Option<String>,
) -> Result<ExtensionActionResultDto, String> {
    let cwd = normalize_cwd(cwd);
    let item = extensions::refresh::load_cached_catalog(&state, provider_id.trim(), cwd.as_deref())
        .await
        .map_err(err_to_string)?
        .items
        .into_iter()
        .find(|item| item.kind == kind.trim() && item.id == extension_id.trim())
        .ok_or_else(|| "extension not found in the cached catalog".to_string())?;
    let result = extensions::perform_action_for_item(
        provider_id.trim(),
        item,
        action.trim(),
        scope.as_deref(),
        cwd.as_deref(),
    )
    .await
    .map_err(err_to_string)?;
    if extensions::refresh::request_catalog_refresh(
        app,
        state.inner().clone(),
        result.provider_id.as_str(),
        cwd,
        extensions::refresh::affected_refresh_kinds(&result.kind),
    )
    .await
    .is_err()
    {
        log::warn!("failed to queue extension catalog refresh after an extension action");
    }
    Ok(result)
}

fn normalize_cwd(cwd: Option<String>) -> Option<String> {
    cwd.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn err_to_string(error: impl std::fmt::Display) -> String {
    format!("{error:#}")
}
