use tauri::{AppHandle, State};

use crate::update::{UpdateManager, UpdateProcessState};

#[tauri::command]
pub fn get_update_state(
    app: AppHandle,
    manager: State<'_, UpdateManager>,
) -> Result<UpdateProcessState, String> {
    manager.restore(&app)
}

#[tauri::command]
pub fn is_update_downloaded(
    app: AppHandle,
    manager: State<'_, UpdateManager>,
) -> Result<bool, String> {
    manager.is_downloaded(&app)
}

#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    manager: State<'_, UpdateManager>,
    source: String,
) -> Result<UpdateProcessState, String> {
    manager.check_for_update(&app, &source).await
}

#[tauri::command]
pub async fn download_update(
    app: AppHandle,
    manager: State<'_, UpdateManager>,
    source: String,
) -> Result<UpdateProcessState, String> {
    manager.download_update(&app, &source).await
}

#[tauri::command]
pub fn install_downloaded_update(
    app: AppHandle,
    manager: State<'_, UpdateManager>,
) -> Result<(), String> {
    manager.install_downloaded_update(&app)
}
