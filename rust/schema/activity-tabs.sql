CREATE TABLE telesink_activity_tabs (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE, source_id INTEGER REFERENCES sinks(id) ON DELETE CASCADE, name TEXT NOT NULL, filters TEXT NOT NULL CHECK(json_valid(filters)), position INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE INDEX index_activity_tabs_on_user_position ON telesink_activity_tabs(user_id,position,id);

INSERT INTO telesink_activity_tabs(user_id,source_id,name,filters,position,created_at,updated_at)
SELECT v.user_id,v.sink_id,v.name,json_object('event_type',v.event_type,'event_date',v.event_date,'search_query',v.search_query,'property_key',v.property_key,'property_op',v.property_op,'property_value',v.property_value),v.id,v.created_at,v.updated_at
FROM saved_views v JOIN users u ON u.id=v.user_id JOIN sinks s ON s.id=v.sink_id AND s.account_id=u.account_id
WHERE EXISTS(SELECT 1 FROM sink_memberships m WHERE m.user_id=v.user_id AND m.sink_id=v.sink_id);
