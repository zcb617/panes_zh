CREATE TABLE IF NOT EXISTS extension_catalog_snapshots (
  provider_id TEXT NOT NULL,
  context_key TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('skill', 'plugin', 'mcp')),
  items_json TEXT NOT NULL,
  fetched_at TEXT,
  last_attempt_at TEXT,
  next_refresh_at TEXT,
  last_error TEXT,
  failure_count INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (provider_id, context_key, kind)
);

CREATE INDEX IF NOT EXISTS idx_extension_catalog_snapshots_due
  ON extension_catalog_snapshots(next_refresh_at);
