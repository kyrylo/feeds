CREATE TABLE telesink_api_keys (user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE, token TEXT NOT NULL UNIQUE, created_at TEXT NOT NULL);
CREATE TABLE telesink_activity_visits (user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE, last_seen_id INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL);
CREATE INDEX index_events_on_occurred_at_and_id ON events(occurred_at DESC,id DESC);
