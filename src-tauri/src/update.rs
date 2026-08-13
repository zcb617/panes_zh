use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

const STATE_FILE: &str = "pending-update.json";
const STATE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UpdatePhase {
    Idle,
    Checking,
    Available,
    Downloading,
    Downloaded,
    Installing,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProcessState {
    pub phase: UpdatePhase,
    pub version: Option<String>,
    pub source: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}

impl Default for UpdateProcessState {
    fn default() -> Self {
        Self {
            phase: UpdatePhase::Idle,
            version: None,
            source: None,
            downloaded_bytes: 0,
            total_bytes: None,
            error: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedUpdate {
    state_version: u32,
    version: String,
    source: String,
    file_name: String,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
}

struct RuntimeState {
    state: UpdateProcessState,
    candidate: Option<Update>,
}

pub struct UpdateManager {
    runtime: Mutex<RuntimeState>,
}

impl Default for UpdateManager {
    fn default() -> Self {
        Self {
            runtime: Mutex::new(RuntimeState {
                state: UpdateProcessState::default(),
                candidate: None,
            }),
        }
    }
}

impl UpdateManager {
    pub fn state(&self) -> UpdateProcessState {
        self.runtime
            .lock()
            .expect("update manager lock poisoned")
            .state
            .clone()
    }

    pub fn set_state(&self, state: UpdateProcessState) {
        self.runtime
            .lock()
            .expect("update manager lock poisoned")
            .state = state;
    }

    fn set_candidate(&self, candidate: Option<Update>) {
        self.runtime
            .lock()
            .expect("update manager lock poisoned")
            .candidate = candidate;
    }

    fn candidate(&self) -> Option<Update> {
        self.runtime
            .lock()
            .expect("update manager lock poisoned")
            .candidate
            .clone()
    }

    fn set_progress(&self, downloaded_bytes: u64, total_bytes: Option<u64>) {
        let mut runtime = self.runtime.lock().expect("update manager lock poisoned");
        runtime.state.phase = UpdatePhase::Downloading;
        runtime.state.downloaded_bytes = downloaded_bytes;
        runtime.state.total_bytes = total_bytes;
        runtime.state.error = None;
    }

    fn set_error(
        &self,
        version: Option<String>,
        source: &str,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        error: String,
    ) {
        self.set_state(UpdateProcessState {
            phase: UpdatePhase::Error,
            version,
            source: Some(source.to_string()),
            downloaded_bytes,
            total_bytes,
            error: Some(error),
        });
    }

    pub fn restore(&self, app: &AppHandle) -> Result<UpdateProcessState, String> {
        let current = self.state();
        if matches!(
            current.phase,
            UpdatePhase::Checking | UpdatePhase::Downloading | UpdatePhase::Installing
        ) || (current.phase == UpdatePhase::Available && self.candidate().is_some())
        {
            return Ok(current);
        }

        let saved = match read_saved_update(app) {
            Ok(saved) => saved,
            Err(error) => {
                log::warn!("failed to read saved update state: {error}");
                let _ = clear_saved_update(app, None);
                None
            }
        };

        let Some(saved) = saved else {
            self.set_candidate(None);
            self.set_state(UpdateProcessState::default());
            return Ok(UpdateProcessState::default());
        };

        let current_version = app.package_info().version.to_string();
        if compare_versions(&current_version, &saved.version) != Ordering::Less {
            clear_saved_update(app, Some(&saved))?;
            self.set_candidate(None);
            self.set_state(UpdateProcessState::default());
            return Ok(UpdateProcessState::default());
        }

        if !saved_file_path(app, &saved)?.is_file() {
            clear_saved_update(app, Some(&saved))?;
            self.set_candidate(None);
            self.set_state(UpdateProcessState::default());
            return Ok(UpdateProcessState::default());
        }

        let state = UpdateProcessState {
            phase: UpdatePhase::Downloaded,
            version: Some(saved.version),
            source: Some(saved.source),
            downloaded_bytes: saved.downloaded_bytes,
            total_bytes: saved.total_bytes,
            error: None,
        };
        self.set_candidate(None);
        self.set_state(state.clone());
        Ok(state)
    }

    pub async fn check_for_update(
        &self,
        app: &AppHandle,
        source: &str,
    ) -> Result<UpdateProcessState, String> {
        let current = self.state();
        if matches!(
            current.phase,
            UpdatePhase::Checking
                | UpdatePhase::Downloading
                | UpdatePhase::Downloaded
                | UpdatePhase::Installing
        ) || (current.phase == UpdatePhase::Available && self.candidate().is_some())
        {
            return Ok(current);
        }

        let restored = self.restore(app)?;
        if restored.phase == UpdatePhase::Downloaded {
            return Ok(restored);
        }

        self.set_state(UpdateProcessState {
            phase: UpdatePhase::Checking,
            version: None,
            source: Some(source.to_string()),
            downloaded_bytes: 0,
            total_bytes: None,
            error: None,
        });

        let updater = match app.updater() {
            Ok(updater) => updater,
            Err(error) => {
                let message = error.to_string();
                self.set_error(None, source, 0, None, message.clone());
                return Err(message);
            }
        };
        let update = match updater.check().await {
            Ok(update) => update,
            Err(error) => {
                let message = error.to_string();
                self.set_error(None, source, 0, None, message.clone());
                return Err(message);
            }
        };

        let Some(update) = update else {
            let state = UpdateProcessState::default();
            self.set_candidate(None);
            self.set_state(state.clone());
            return Ok(state);
        };

        let state = UpdateProcessState {
            phase: UpdatePhase::Available,
            version: Some(update.version.clone()),
            source: Some(source.to_string()),
            downloaded_bytes: 0,
            total_bytes: None,
            error: None,
        };
        self.set_candidate(Some(update));
        self.set_state(state.clone());
        Ok(state)
    }

    pub async fn download_update(
        &self,
        app: &AppHandle,
        source: &str,
    ) -> Result<UpdateProcessState, String> {
        let current_state = self.state();
        if matches!(
            current_state.phase,
            UpdatePhase::Downloading | UpdatePhase::Downloaded | UpdatePhase::Installing
        ) {
            return Ok(current_state);
        }

        let update = self
            .candidate()
            .ok_or_else(|| "没有可下载的更新版本".to_string())?;
        let version = update.version.clone();
        let suffix = package_suffix(update.download_url.path());
        self.set_state(UpdateProcessState {
            phase: UpdatePhase::Downloading,
            version: Some(version.clone()),
            source: Some(source.to_string()),
            downloaded_bytes: 0,
            total_bytes: None,
            error: None,
        });

        let mut downloaded_bytes = 0_u64;
        let manager = self;
        let progress_app = app.clone();
        let bytes = match update
            .download(
                |chunk_length, content_length| {
                    downloaded_bytes = downloaded_bytes.saturating_add(chunk_length as u64);
                    manager.set_progress(downloaded_bytes, content_length);
                    let _ = progress_app.emit("update-download-progress", manager.state());
                },
                || {},
            )
            .await
        {
            Ok(bytes) => bytes,
            Err(error) => {
                let message = error.to_string();
                self.set_error(
                    Some(version),
                    source,
                    downloaded_bytes,
                    self.state().total_bytes,
                    message.clone(),
                );
                return Err(message);
            }
        };

        let saved = match save_downloaded_update(
            app,
            &version,
            source,
            &suffix,
            &bytes,
            downloaded_bytes,
            self.state().total_bytes,
        ) {
            Ok(saved) => saved,
            Err(error) => {
                self.set_error(
                    Some(version),
                    source,
                    downloaded_bytes,
                    self.state().total_bytes,
                    error.clone(),
                );
                return Err(error);
            }
        };
        let state = UpdateProcessState {
            phase: UpdatePhase::Downloaded,
            version: Some(saved.version),
            source: Some(saved.source),
            downloaded_bytes: saved.downloaded_bytes,
            total_bytes: saved.total_bytes,
            error: None,
        };
        self.set_state(state.clone());
        Ok(state)
    }

    pub fn install_downloaded_update(&self, app: &AppHandle) -> Result<(), String> {
        let saved = read_saved_update(app)?.ok_or_else(|| "没有已下载完成的更新".to_string())?;
        let file_path = saved_file_path(app, &saved)?;
        let bytes = fs::read(&file_path).map_err(|error| error.to_string())?;
        if bytes.is_empty() {
            return Err("已下载的更新文件为空".to_string());
        }

        self.set_state(UpdateProcessState {
            phase: UpdatePhase::Installing,
            version: Some(saved.version.clone()),
            source: Some(saved.source.clone()),
            downloaded_bytes: saved.downloaded_bytes,
            total_bytes: saved.total_bytes,
            error: None,
        });

        let result = if let Some(update) = self.candidate() {
            update.install(bytes).map_err(|error| error.to_string())
        } else {
            install_saved_file(&file_path, &saved.file_name)
        };
        if let Err(error) = result {
            self.set_error(
                Some(saved.version),
                &saved.source,
                saved.downloaded_bytes,
                saved.total_bytes,
                error.clone(),
            );
            return Err(error);
        }
        Ok(())
    }

    pub fn is_downloaded(&self, app: &AppHandle) -> Result<bool, String> {
        if self.state().phase == UpdatePhase::Downloaded {
            return Ok(true);
        }
        Ok(self.restore(app)?.phase == UpdatePhase::Downloaded)
    }
}

fn updates_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("updates"))
        .map_err(|error| error.to_string())
}

fn state_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(updates_dir(app)?.join(STATE_FILE))
}

fn saved_file_path(app: &AppHandle, saved: &SavedUpdate) -> Result<PathBuf, String> {
    let file_name = Path::new(&saved.file_name)
        .file_name()
        .ok_or_else(|| "更新文件名无效".to_string())?;
    Ok(updates_dir(app)?.join(file_name))
}

fn read_saved_update(app: &AppHandle) -> Result<Option<SavedUpdate>, String> {
    let path = state_path(app)?;
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let saved = serde_json::from_str::<SavedUpdate>(&text).map_err(|error| error.to_string())?;
    if saved.state_version != STATE_VERSION {
        return Ok(None);
    }
    Ok(Some(saved))
}

fn save_downloaded_update(
    app: &AppHandle,
    version: &str,
    source: &str,
    suffix: &str,
    bytes: &[u8],
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
) -> Result<SavedUpdate, String> {
    let directory = updates_dir(app)?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let file_name = format!("pending-update{suffix}");
    let file_path = directory.join(&file_name);
    atomic_write(&file_path, bytes)?;

    let saved = SavedUpdate {
        state_version: STATE_VERSION,
        version: version.to_string(),
        source: source.to_string(),
        file_name,
        downloaded_bytes: if downloaded_bytes == 0 {
            bytes.len() as u64
        } else {
            downloaded_bytes
        },
        total_bytes,
    };
    let text = serde_json::to_vec_pretty(&saved).map_err(|error| error.to_string())?;
    atomic_write(&state_path(app)?, &text)?;
    Ok(saved)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "更新文件名无效".to_string())?;
    let temporary = path.with_file_name(format!(".{name}.tmp"));
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

fn clear_saved_update(app: &AppHandle, saved: Option<&SavedUpdate>) -> Result<(), String> {
    if let Some(saved) = saved {
        if let Ok(path) = saved_file_path(app, saved) {
            let _ = fs::remove_file(path);
        }
    }
    if let Ok(directory) = updates_dir(app) {
        for suffix in [
            ".app.tar.gz",
            ".appimage",
            ".exe",
            ".msi",
            ".deb",
            ".rpm",
            ".bin",
        ] {
            let _ = fs::remove_file(directory.join(format!("pending-update{suffix}")));
        }
    }
    let _ = fs::remove_file(state_path(app)?);
    Ok(())
}

fn package_suffix(path: &str) -> String {
    let path = path.to_ascii_lowercase();
    [".app.tar.gz", ".appimage", ".exe", ".msi", ".deb", ".rpm"]
        .iter()
        .find(|suffix| path.ends_with(**suffix))
        .unwrap_or(&".bin")
        .to_string()
}

fn compare_versions(left: &str, right: &str) -> Ordering {
    let left = version_parts(left);
    let right = version_parts(right);
    let length = left.len().max(right.len());
    (0..length)
        .map(|index| {
            (
                left.get(index).copied().unwrap_or(0),
                right.get(index).copied().unwrap_or(0),
            )
        })
        .find_map(|(left, right)| match left.cmp(&right) {
            Ordering::Equal => None,
            ordering => Some(ordering),
        })
        .unwrap_or(Ordering::Equal)
}

fn version_parts(version: &str) -> Vec<u64> {
    version
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

fn install_saved_file(path: &Path, file_name: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        if file_name.ends_with(".exe") {
            let mut installer = Command::new(path);
            installer.args(["/P", "/R", "/UPDATE", "/ARGS"]);
            installer.args(std::env::args_os().skip(1));
            installer.spawn().map_err(|error| error.to_string())?;
        } else if file_name.ends_with(".msi") {
            let mut installer = Command::new("msiexec.exe");
            installer.args([
                "/i",
                path.to_string_lossy().as_ref(),
                "/passive",
                "/promptrestart",
                "AUTOLAUNCHAPP=True",
            ]);
            installer.spawn().map_err(|error| error.to_string())?;
        } else {
            return Err("当前 Windows 更新包格式不受支持".to_string());
        }
        std::process::exit(0);
    }

    #[cfg(target_os = "macos")]
    {
        return install_macos_file(path);
    }

    #[cfg(target_os = "linux")]
    {
        if file_name.ends_with(".deb") {
            Command::new("pkexec")
                .args(["dpkg", "-i", path.to_string_lossy().as_ref()])
                .spawn()
                .map_err(|error| error.to_string())?;
            std::process::exit(0);
        }
        if file_name.ends_with(".rpm") {
            Command::new("pkexec")
                .args(["rpm", "-U", path.to_string_lossy().as_ref()])
                .spawn()
                .map_err(|error| error.to_string())?;
            std::process::exit(0);
        }
        let current = std::env::current_exe().map_err(|error| error.to_string())?;
        fs::copy(path, current).map_err(|error| error.to_string())?;
        std::process::exit(0);
    }

    #[allow(unreachable_code)]
    Err("当前平台不支持持久化更新安装".to_string())
}

#[cfg(target_os = "macos")]
fn install_macos_file(path: &Path) -> Result<(), String> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let app_dir = current
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| "无法定位当前 macOS 应用目录".to_string())?
        .to_path_buf();
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let mut decoder = GzDecoder::new(bytes.as_slice());
    let mut tar_bytes = Vec::new();
    decoder
        .read_to_end(&mut tar_bytes)
        .map_err(|error| error.to_string())?;
    let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
    tar::Archive::new(tar_bytes.as_slice())
        .unpack(temporary.path())
        .map_err(|error| error.to_string())?;
    let replacement = find_app_dir(temporary.path())
        .ok_or_else(|| "更新文件中没有找到 macOS 应用".to_string())?;
    let backup = app_dir.with_extension("old");
    let _ = fs::remove_dir_all(&backup);
    fs::rename(&app_dir, &backup).map_err(|error| error.to_string())?;
    if let Err(error) = fs::rename(&replacement, &app_dir) {
        let _ = fs::rename(&backup, &app_dir);
        return Err(error.to_string());
    }
    let _ = fs::remove_dir_all(backup);
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
fn find_app_dir(root: &Path) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()? {
        let path = entry.ok()?.path();
        if path.extension().is_some_and(|extension| extension == "app") {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find_app_dir(&path) {
                return Some(found);
            }
        }
    }
    None
}
