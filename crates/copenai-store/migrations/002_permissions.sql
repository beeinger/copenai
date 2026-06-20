CREATE TABLE IF NOT EXISTS permission_requests (
    id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL,
    tool_title TEXT NOT NULL,
    options_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    decision_option_id TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_permission_requests_conv_status
    ON permission_requests (conversation_id, status);
