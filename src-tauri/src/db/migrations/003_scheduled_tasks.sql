CREATE TABLE IF NOT EXISTS scheduled_tasks (
  id TEXT PRIMARY KEY,
  description TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  execution_device_id TEXT NOT NULL DEFAULT 'local',
  target_type TEXT NOT NULL,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  thread_id TEXT REFERENCES threads(id) ON DELETE SET NULL,
  runtime_config_json TEXT,
  schedule_type TEXT NOT NULL,
  schedule_json TEXT NOT NULL,
  timezone TEXT NOT NULL,
  next_run_at TEXT,
  last_run_at TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  CHECK (target_type IN ('existing_thread', 'new_thread')),
  CHECK (schedule_type IN ('interval', 'daily', 'weekly'))
);

CREATE TABLE IF NOT EXISTS scheduled_task_runs (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES scheduled_tasks(id) ON DELETE CASCADE,
  scheduled_for TEXT NOT NULL,
  started_at TEXT,
  finished_at TEXT,
  thread_id TEXT REFERENCES threads(id) ON DELETE SET NULL,
  assistant_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
  status TEXT NOT NULL DEFAULT 'queued',
  error_message TEXT,
  result_preview TEXT,
  acknowledged_at TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  CHECK (status IN (
    'queued',
    'running',
    'needs_confirmation',
    'completed',
    'error',
    'interrupted',
    'skipped'
  )),
  UNIQUE(task_id, scheduled_for)
);

CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_due
  ON scheduled_tasks(enabled, next_run_at);
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_workspace
  ON scheduled_tasks(workspace_id);
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_thread
  ON scheduled_tasks(thread_id);
CREATE INDEX IF NOT EXISTS idx_scheduled_task_runs_task_created
  ON scheduled_task_runs(task_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_scheduled_task_runs_message
  ON scheduled_task_runs(assistant_message_id);
