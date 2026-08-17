use std::path::Path;

use anyhow::Context;
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::models::WorkspaceDto;
use crate::path_utils;
use crate::runtime_env;

use super::Database;

const DEFAULT_SCAN_DEPTH: i64 = 3;

pub fn upsert_workspace(
    db: &Database,
    root_path: &str,
    scan_depth: Option<i64>,
) -> anyhow::Result<WorkspaceDto> {
    let conn = db.connect()?;
    let canonical_path = path_utils::canonicalize_path(Path::new(root_path))
        .unwrap_or_else(|_| path_utils::normalize_windows_path(Path::new(root_path).to_path_buf()));
    let canonical = canonical_path.to_string_lossy().to_string();
    let legacy_canonical = path_utils::legacy_windows_verbatim_path(&canonical_path)
        .filter(|legacy| legacy != &canonical);

    let existing = if let Some(id) = find_workspace_id_by_root(&conn, &canonical)? {
        Some(id)
    } else if let Some(legacy_canonical) = legacy_canonical.as_deref() {
        find_workspace_id_by_root(&conn, legacy_canonical)?
    } else {
        None
    };

    if let Some(id) = existing {
        conn.execute(
            "UPDATE workspaces
       SET root_path = ?2,
           last_opened_at = datetime('now'),
           scan_depth = COALESCE(?3, scan_depth),
           archived_at = NULL
       WHERE id = ?1",
            params![id, canonical, scan_depth],
        )
        .context("failed to update workspace last_opened_at")?;
    } else {
        let id = Uuid::new_v4().to_string();
        let name = workspace_name_from_path(&canonical);
        let scan_depth = scan_depth.unwrap_or(DEFAULT_SCAN_DEPTH);
        conn.execute(
            "INSERT INTO workspaces (
                id, name, root_path, location_kind, ssh_connection_id, scan_depth
             ) VALUES (?1, ?2, ?3, 'local', NULL, ?4)",
            params![id, name, canonical, scan_depth],
        )
        .context("failed to insert workspace")?;
    }

    get_workspace_by_root(&conn, &canonical)
}

pub fn create_ssh_workspace(
    db: &Database,
    connection_id: &str,
    name: &str,
    root_path: &str,
    scan_depth: Option<i64>,
) -> anyhow::Result<WorkspaceDto> {
    let root_path = root_path.trim();
    if !root_path.starts_with('/') || root_path.contains('\0') {
        anyhow::bail!("远端目录必须是绝对路径");
    }
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("项目名称不能为空");
    }

    let conn = db.connect()?;
    let connection_state = conn
        .query_row(
            "SELECT enabled, connection_status, deleted_at FROM ssh_connections WHERE id = ?1",
            params![connection_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? > 0,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .context("failed to load ssh connection for workspace")?
        .ok_or_else(|| anyhow::anyhow!("SSH 连接不存在"))?;
    if connection_state.2.is_some() {
        anyhow::bail!("SSH 连接已删除，请先恢复连接");
    }
    if !connection_state.0 {
        anyhow::bail!("SSH 连接已禁用，无法创建远端项目");
    }
    if connection_state.1 != crate::db::ssh_connections::STATUS_OK {
        anyhow::bail!("SSH 连接尚未连接成功，无法创建远端项目");
    }

    let scan_depth = scan_depth.unwrap_or(DEFAULT_SCAN_DEPTH);
    let existing = conn
        .query_row(
            "SELECT id, archived_at
             FROM workspaces
             WHERE location_kind = 'ssh'
               AND ssh_connection_id = ?1
               AND root_path = ?2",
            params![connection_id, root_path],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .context("failed to query remote workspace")?;

    let workspace_id = if let Some((workspace_id, archived_at)) = existing {
        if archived_at.is_none() {
            return get_workspace_by_id(&conn, &workspace_id);
        }
        conn.execute(
            "UPDATE workspaces
             SET name = ?1,
                 scan_depth = ?2,
                 archived_at = NULL,
                 last_opened_at = datetime('now')
             WHERE id = ?3",
            params![name, scan_depth, workspace_id],
        )
        .context("failed to restore remote workspace")?;
        workspace_id
    } else {
        let workspace_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO workspaces (
               id, name, root_path, location_kind, ssh_connection_id, scan_depth
             ) VALUES (?1, ?2, ?3, 'ssh', ?4, ?5)",
            params![workspace_id, name, root_path, connection_id, scan_depth],
        )
        .context("failed to insert remote workspace")?;
        workspace_id
    };

    get_workspace_by_id(&conn, &workspace_id)
}

pub fn list_workspaces(db: &Database) -> anyhow::Result<Vec<WorkspaceDto>> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT w.id, w.name, w.root_path, w.scan_depth, w.created_at, w.last_opened_at,
                w.location_kind, w.ssh_connection_id, s.display_name, s.enabled, s.deleted_at,
                s.connection_status
         FROM workspaces w
         LEFT JOIN ssh_connections s ON s.id = w.ssh_connection_id
         WHERE w.archived_at IS NULL
           AND (w.location_kind = 'local' OR (s.id IS NOT NULL AND s.deleted_at IS NULL))
         ORDER BY w.last_opened_at DESC",
    )?;

    let rows = stmt.query_map([], map_workspace_row)?;
    let mut out = Vec::new();

    for item in rows {
        out.push(item?);
    }

    Ok(out)
}

pub fn list_archived_workspaces(db: &Database) -> anyhow::Result<Vec<WorkspaceDto>> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT w.id, w.name, w.root_path, w.scan_depth, w.created_at, w.last_opened_at,
                w.location_kind, w.ssh_connection_id, s.display_name, s.enabled, s.deleted_at,
                s.connection_status
         FROM workspaces w
         LEFT JOIN ssh_connections s ON s.id = w.ssh_connection_id
         WHERE w.archived_at IS NOT NULL
           AND (w.location_kind = 'local' OR (s.id IS NOT NULL AND s.deleted_at IS NULL))
         ORDER BY w.archived_at DESC",
    )?;

    let rows = stmt.query_map([], map_workspace_row)?;
    let mut out = Vec::new();

    for item in rows {
        out.push(item?);
    }

    Ok(out)
}

pub fn ensure_default_workspace(db: &Database) -> anyhow::Result<WorkspaceDto> {
    let current_exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let temp_dir = std::env::temp_dir();
    let windows_dir = windows_system_dir();
    if let Some(first) = list_workspaces(db)?.into_iter().find(|workspace| {
        workspace.location_kind == "local"
            && is_viable_workspace_root(
                Path::new(&workspace.root_path),
                current_exe_dir.as_deref(),
                cfg!(target_os = "windows"),
                Some(temp_dir.as_path()),
                windows_dir.as_deref(),
            )
    }) {
        return Ok(first);
    }

    let root = preferred_default_workspace_root();
    let root = root.to_string_lossy().to_string();
    upsert_workspace(db, &root, None)
}

fn preferred_default_workspace_root() -> std::path::PathBuf {
    let cwd = std::env::current_dir().ok();
    let home = runtime_env::home_dir();
    let current_exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let temp_dir = std::env::temp_dir();
    let windows_dir = windows_system_dir();
    preferred_default_workspace_root_for(
        cwd.as_deref(),
        home.as_deref(),
        current_exe_dir.as_deref(),
        cfg!(target_os = "windows"),
        Some(temp_dir.as_path()),
        windows_dir.as_deref(),
    )
}

fn preferred_default_workspace_root_for(
    cwd: Option<&Path>,
    home: Option<&Path>,
    current_exe_dir: Option<&Path>,
    is_windows: bool,
    temp_dir: Option<&Path>,
    windows_dir: Option<&Path>,
) -> std::path::PathBuf {
    cwd.filter(|path| {
        is_viable_workspace_root(path, current_exe_dir, is_windows, temp_dir, windows_dir)
    })
    .or_else(|| {
        home.filter(|path| {
            is_viable_workspace_root(path, current_exe_dir, is_windows, temp_dir, windows_dir)
        })
    })
    .map(Path::to_path_buf)
    .unwrap_or_else(|| std::path::PathBuf::from("."))
}

fn is_viable_workspace_root(
    path: &Path,
    current_exe_dir: Option<&Path>,
    is_windows: bool,
    temp_dir: Option<&Path>,
    windows_dir: Option<&Path>,
) -> bool {
    path.is_dir()
        && !is_transient_appimage_mount(path)
        && !is_current_executable_tree(path, current_exe_dir)
        && !is_unsafe_windows_default_root(path, is_windows, temp_dir, windows_dir)
}

fn is_transient_appimage_mount(path: &Path) -> bool {
    let rendered = path.to_string_lossy();
    rendered.starts_with("/tmp/.mount_") || rendered.starts_with("/var/tmp/.mount_")
}

fn is_current_executable_tree(path: &Path, current_exe_dir: Option<&Path>) -> bool {
    current_exe_dir.is_some_and(|dir| path == dir || path.starts_with(dir))
}

fn is_unsafe_windows_default_root(
    path: &Path,
    is_windows: bool,
    temp_dir: Option<&Path>,
    windows_dir: Option<&Path>,
) -> bool {
    if !is_windows {
        return false;
    }

    temp_dir.is_some_and(|dir| path.starts_with(dir))
        || windows_dir.is_some_and(|dir| path.starts_with(dir))
}

fn windows_system_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

pub fn delete_workspace(db: &Database, workspace_id: &str) -> anyhow::Result<()> {
    let conn = db.connect()?;
    let affected = conn
        .execute(
            "DELETE FROM workspaces WHERE id = ?1",
            params![workspace_id],
        )
        .context("failed to delete workspace")?;

    if affected == 0 {
        anyhow::bail!("workspace not found: {workspace_id}");
    }

    Ok(())
}

pub fn archive_workspace(db: &Database, workspace_id: &str) -> anyhow::Result<()> {
    let conn = db.connect()?;
    let affected = conn
        .execute(
            "UPDATE workspaces
       SET archived_at = datetime('now')
       WHERE id = ?1
         AND archived_at IS NULL",
            params![workspace_id],
        )
        .context("failed to archive workspace")?;

    if affected == 0 {
        anyhow::bail!("workspace not found or already archived: {workspace_id}");
    }

    Ok(())
}

pub fn restore_workspace(db: &Database, workspace_id: &str) -> anyhow::Result<WorkspaceDto> {
    let conn = db.connect()?;
    let affected = conn
        .execute(
            "UPDATE workspaces
       SET archived_at = NULL,
           last_opened_at = datetime('now')
       WHERE id = ?1
         AND archived_at IS NOT NULL
         AND (
           location_kind = 'local'
           OR EXISTS (
             SELECT 1 FROM ssh_connections s
             WHERE s.id = workspaces.ssh_connection_id
               AND s.deleted_at IS NULL
           )
         )",
            params![workspace_id],
        )
        .context("failed to restore workspace")?;

    if affected == 0 {
        anyhow::bail!("workspace not found or not archived: {workspace_id}");
    }

    get_workspace_by_id(&conn, workspace_id)
}

pub fn find_workspace_by_id(
    db: &Database,
    workspace_id: &str,
) -> anyhow::Result<Option<WorkspaceDto>> {
    let conn = db.connect()?;
    get_workspace_by_id_optional(&conn, workspace_id)
}

pub fn workspace_ids_for_ssh_connection(
    db: &Database,
    connection_id: &str,
) -> anyhow::Result<Vec<String>> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id FROM workspaces
         WHERE location_kind = 'ssh' AND ssh_connection_id = ?1",
    )?;
    let rows = stmt.query_map(params![connection_id], |row| row.get::<_, String>(0))?;
    rows.collect::<Result<Vec<_>, _>>()
        .context("failed to load workspaces for SSH connection")
}

pub fn get_workspace_startup_preset_json(
    db: &Database,
    workspace_id: &str,
) -> anyhow::Result<Option<String>> {
    let conn = db.connect()?;
    conn.query_row(
        "SELECT startup_preset_json
         FROM workspaces
         WHERE id = ?1",
        params![workspace_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .optional()
    .context("failed to load workspace startup preset")
    .map(|value| value.flatten())
}

pub fn set_workspace_startup_preset_json(
    db: &Database,
    workspace_id: &str,
    startup_preset_json: Option<&str>,
) -> anyhow::Result<()> {
    let conn = db.connect()?;
    let affected = conn
        .execute(
            "UPDATE workspaces
             SET startup_preset_json = ?1,
                 startup_preset_updated_at = CASE
                     WHEN ?1 IS NULL THEN NULL
                     ELSE datetime('now')
                 END
             WHERE id = ?2",
            params![startup_preset_json, workspace_id],
        )
        .context("failed to persist workspace startup preset")?;

    if affected == 0 {
        anyhow::bail!("workspace not found: {workspace_id}");
    }

    Ok(())
}

pub fn is_git_repo_selection_configured(db: &Database, workspace_id: &str) -> anyhow::Result<bool> {
    let conn = db.connect()?;
    let configured = conn
        .query_row(
            "SELECT git_repo_selection_configured
         FROM workspaces
         WHERE id = ?1",
            params![workspace_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .context("failed to load workspace git selection state")?;

    Ok(configured.unwrap_or(0) > 0)
}

pub fn set_git_repo_selection_configured(
    db: &Database,
    workspace_id: &str,
    configured: bool,
) -> anyhow::Result<()> {
    let conn = db.connect()?;
    let affected = conn
        .execute(
            "UPDATE workspaces
         SET git_repo_selection_configured = ?1
         WHERE id = ?2",
            params![if configured { 1 } else { 0 }, workspace_id],
        )
        .context("failed to update workspace git selection state")?;

    if affected == 0 {
        anyhow::bail!("workspace not found: {workspace_id}");
    }

    Ok(())
}

fn get_workspace_by_root(
    conn: &rusqlite::Connection,
    root_path: &str,
) -> anyhow::Result<WorkspaceDto> {
    conn.query_row(
        "SELECT w.id, w.name, w.root_path, w.scan_depth, w.created_at, w.last_opened_at,
                w.location_kind, w.ssh_connection_id, s.display_name, s.enabled, s.deleted_at,
                s.connection_status
         FROM workspaces w
         LEFT JOIN ssh_connections s ON s.id = w.ssh_connection_id
         WHERE w.root_path = ?1 AND w.location_kind = 'local'",
        params![root_path],
        map_workspace_row,
    )
    .context("failed to load workspace by root")
}

fn find_workspace_id_by_root(
    conn: &rusqlite::Connection,
    root_path: &str,
) -> anyhow::Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM workspaces
         WHERE root_path = ?1 AND location_kind = 'local'",
        params![root_path],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .context("failed to query workspace")
}

fn get_workspace_by_id(
    conn: &rusqlite::Connection,
    workspace_id: &str,
) -> anyhow::Result<WorkspaceDto> {
    get_workspace_by_id_optional(conn, workspace_id)?
        .ok_or_else(|| anyhow::anyhow!("workspace not found: {workspace_id}"))
}

fn get_workspace_by_id_optional(
    conn: &rusqlite::Connection,
    workspace_id: &str,
) -> anyhow::Result<Option<WorkspaceDto>> {
    conn.query_row(
        "SELECT w.id, w.name, w.root_path, w.scan_depth, w.created_at, w.last_opened_at,
                w.location_kind, w.ssh_connection_id, s.display_name, s.enabled, s.deleted_at,
                s.connection_status
         FROM workspaces w
         LEFT JOIN ssh_connections s ON s.id = w.ssh_connection_id
         WHERE w.id = ?1",
        params![workspace_id],
        map_workspace_row,
    )
    .optional()
    .context("failed to load workspace by id")
}

fn workspace_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "workspace".to_string())
}

fn map_workspace_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceDto> {
    let location_kind = row.get::<_, String>(6)?;
    let raw_root_path = row.get::<_, String>(2)?;
    let root_path = if location_kind == "local" {
        path_utils::normalize_windows_path_string(&raw_root_path)
    } else {
        raw_root_path
    };
    let ssh_connection_id = row.get::<_, Option<String>>(7)?;
    let connection_enabled = row.get::<_, Option<i64>>(9)?.map(|value| value > 0);
    let connection_deleted_at = row.get::<_, Option<String>>(10)?;
    Ok(WorkspaceDto {
        id: row.get(0)?,
        name: row.get(1)?,
        root_path,
        location_kind,
        ssh_connection_id: ssh_connection_id.clone(),
        connection_display_name: row.get(8)?,
        connection_enabled,
        connection_deleted: ssh_connection_id.map(|_| connection_deleted_at.is_some()),
        connection_status: row.get(11)?,
        scan_depth: row.get(3)?,
        created_at: row.get(4)?,
        last_opened_at: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{Arc, Mutex},
    };

    use uuid::Uuid;

    use crate::db::{ConnectionPool, SQLITE_POOL_MAX_IDLE};

    use super::*;

    fn test_db() -> Database {
        let path = std::env::temp_dir().join(format!("panes-workspaces-{}.db", Uuid::new_v4()));
        let db = Database {
            path,
            pool: Arc::new(ConnectionPool {
                idle: Mutex::new(Vec::new()),
                max_idle: SQLITE_POOL_MAX_IDLE,
            }),
        };
        db.run_migrations().expect("failed to run test migrations");
        db
    }

    fn insert_test_connection(db: &Database, id: &str, display_name: &str) {
        let conn = db.connect().expect("failed to open test connection");
        conn.execute(
            "INSERT INTO ssh_connections (
                id, display_name, source_kind, host_name, user_name, port,
                host_key_type, host_key_base64, connection_status
             ) VALUES (?1, ?2, 'manual', '192.0.2.10', 'tester', 22, 'ssh-ed25519', 'test-key', 'ok')",
            params![id, display_name],
        )
        .expect("failed to insert test ssh connection");
    }

    #[test]
    fn remote_workspace_keeps_identity_and_hides_only_when_connection_is_deleted() {
        let db = test_db();
        insert_test_connection(&db, "ssh-a", "Remote A");
        insert_test_connection(&db, "ssh-b", "Remote B");

        let first = create_ssh_workspace(&db, "ssh-a", "Repo", "/home/tester/Repo", Some(4))
            .expect("failed to create remote workspace");
        assert_eq!(first.location_kind, "ssh");
        assert_eq!(first.ssh_connection_id.as_deref(), Some("ssh-a"));
        assert_eq!(first.root_path, "/home/tester/Repo");

        let duplicate = create_ssh_workspace(&db, "ssh-a", "Repo 2", "/home/tester/Repo", None)
            .expect("an existing remote workspace should be reused");
        assert_eq!(duplicate.id, first.id);

        let same_path_other_host =
            create_ssh_workspace(&db, "ssh-b", "Repo", "/home/tester/Repo", None)
                .expect("same path on another host should be allowed");
        assert_ne!(first.id, same_path_other_host.id);

        archive_workspace(&db, &first.id).expect("failed to archive remote workspace");
        let restored = create_ssh_workspace(&db, "ssh-a", "Renamed", "/home/tester/Repo", None)
            .expect("failed to restore archived remote workspace");
        assert_eq!(restored.id, first.id);
        assert_eq!(restored.name, "Renamed");

        crate::db::ssh_connections::soft_delete(&db, "ssh-a")
            .expect("failed to soft delete test connection");
        let visible_after_delete = list_workspaces(&db).expect("failed to list workspaces");
        assert!(visible_after_delete
            .iter()
            .all(|workspace| workspace.id != first.id));

        crate::db::ssh_connections::restore(&db, "ssh-a").expect("failed to restore connection");
        let visible_after_restore = list_workspaces(&db).expect("failed to list workspaces");
        assert!(visible_after_restore
            .iter()
            .any(|workspace| workspace.id == first.id));

        crate::db::ssh_connections::set_enabled(&db, "ssh-a", false)
            .expect("failed to disable test connection");
        let disabled = list_workspaces(&db)
            .expect("failed to list workspaces after disabling connection")
            .into_iter()
            .find(|workspace| workspace.id == first.id)
            .expect("disabled connection project should remain visible");
        assert_eq!(disabled.connection_enabled, Some(false));
    }

    #[test]
    fn lists_every_workspace_bound_to_an_ssh_connection() {
        let db = test_db();
        insert_test_connection(&db, "ssh-a", "Remote A");
        insert_test_connection(&db, "ssh-b", "Remote B");

        let first = create_ssh_workspace(&db, "ssh-a", "Repo A", "/srv/repo-a", None)
            .expect("failed to create first remote workspace");
        let second = create_ssh_workspace(&db, "ssh-a", "Repo B", "/srv/repo-b", None)
            .expect("failed to create second remote workspace");
        create_ssh_workspace(&db, "ssh-b", "Repo C", "/srv/repo-c", None)
            .expect("failed to create unrelated remote workspace");

        let mut workspace_ids = workspace_ids_for_ssh_connection(&db, "ssh-a")
            .expect("failed to list workspaces for connection");
        workspace_ids.sort();
        let mut expected = vec![first.id, second.id];
        expected.sort();
        assert_eq!(workspace_ids, expected);
    }

    #[test]
    fn upsert_workspace_preserves_existing_scan_depth_when_none_is_provided() {
        let db = test_db();
        let root = std::env::temp_dir().join(format!("panes-workspace-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("failed to create temp workspace root");
        let root = root.to_string_lossy().to_string();

        let created = upsert_workspace(&db, &root, Some(7)).expect("failed to create workspace");
        let reopened =
            upsert_workspace(&db, &root, None).expect("failed to reopen workspace without depth");

        assert_eq!(created.id, reopened.id);
        assert_eq!(reopened.scan_depth, 7);
    }

    #[test]
    fn preferred_default_workspace_root_skips_transient_appimage_mounts() {
        let home = std::env::temp_dir().join(format!("panes-home-{}", Uuid::new_v4()));
        fs::create_dir_all(&home).expect("failed to create temp home");

        let cwd = std::path::Path::new("/tmp/.mount_PanesTest/usr");
        let selected =
            preferred_default_workspace_root_for(Some(cwd), Some(&home), None, false, None, None);

        assert_eq!(selected, home);
    }

    #[test]
    fn preferred_default_workspace_root_keeps_existing_directory_cwd() {
        let cwd = std::env::temp_dir().join(format!("panes-cwd-{}", Uuid::new_v4()));
        let home = std::env::temp_dir().join(format!("panes-home-{}", Uuid::new_v4()));
        fs::create_dir_all(&cwd).expect("failed to create temp cwd");
        fs::create_dir_all(&home).expect("failed to create temp home");

        let selected =
            preferred_default_workspace_root_for(Some(&cwd), Some(&home), None, false, None, None);

        assert_eq!(selected, cwd);
    }

    #[test]
    fn preferred_default_workspace_root_skips_current_executable_directory() {
        let cwd = std::env::temp_dir().join(format!("panes-install-{}", Uuid::new_v4()));
        let home = std::env::temp_dir().join(format!("panes-home-{}", Uuid::new_v4()));
        fs::create_dir_all(&cwd).expect("failed to create temp install root");
        fs::create_dir_all(&home).expect("failed to create temp home");

        let selected = preferred_default_workspace_root_for(
            Some(&cwd),
            Some(&home),
            Some(&cwd),
            false,
            None,
            None,
        );

        assert_eq!(selected, home);
    }

    #[test]
    fn preferred_default_workspace_root_keeps_windows_home_when_executable_is_nested_inside_it() {
        let home = std::env::temp_dir()
            .join(format!("panes-home-{}", Uuid::new_v4()))
            .join("Users")
            .join("panes");
        let install_dir = home.join("AppData").join("Local").join("Panes");
        fs::create_dir_all(&install_dir).expect("failed to create fake install dir");
        fs::create_dir_all(&home).expect("failed to create temp home");

        let selected = preferred_default_workspace_root_for(
            Some(&install_dir),
            Some(&home),
            Some(&install_dir),
            true,
            None,
            None,
        );

        assert_eq!(selected, home);
    }

    #[test]
    fn preferred_default_workspace_root_skips_windows_system_dirs() {
        let windows_dir =
            std::env::temp_dir().join(format!("panes-windows-dir-{}", Uuid::new_v4()));
        let home = std::env::temp_dir().join(format!("panes-home-{}", Uuid::new_v4()));
        let cwd = windows_dir.join("System32");
        fs::create_dir_all(&cwd).expect("failed to create fake windows cwd");
        fs::create_dir_all(&home).expect("failed to create temp home");

        let selected = preferred_default_workspace_root_for(
            Some(&cwd),
            Some(&home),
            None,
            true,
            None,
            Some(&windows_dir),
        );

        assert_eq!(selected, home);
    }

    #[test]
    fn preferred_default_workspace_root_skips_windows_temp_dirs() {
        let temp_dir = std::env::temp_dir().join(format!("panes-temp-{}", Uuid::new_v4()));
        let home = std::env::temp_dir().join(format!("panes-home-{}", Uuid::new_v4()));
        let cwd = temp_dir.join("nsis");
        fs::create_dir_all(&cwd).expect("failed to create fake temp cwd");
        fs::create_dir_all(&home).expect("failed to create temp home");

        let selected = preferred_default_workspace_root_for(
            Some(&cwd),
            Some(&home),
            None,
            true,
            Some(&temp_dir),
            None,
        );

        assert_eq!(selected, home);
    }
}
