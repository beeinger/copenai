CREATE TABLE responses (
  id TEXT PRIMARY KEY,
  conversation_id TEXT,
  status TEXT NOT NULL,
  model TEXT NOT NULL,
  tool_execution TEXT NOT NULL DEFAULT 'client',
  request_json TEXT NOT NULL,
  response_json TEXT,
  output_json TEXT,
  usage_json TEXT,
  previous_response_id TEXT,
  created_at TEXT NOT NULL,
  completed_at TEXT
);

CREATE INDEX idx_responses_conversation ON responses(conversation_id);
CREATE INDEX idx_responses_created ON responses(created_at);
