CREATE TABLE telesink_activity_tab_visits (tab_id INTEGER PRIMARY KEY REFERENCES telesink_activity_tabs(id) ON DELETE CASCADE, last_seen_id INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL);
CREATE TRIGGER initialize_activity_tab_visit AFTER INSERT ON telesink_activity_tabs BEGIN INSERT INTO telesink_activity_tab_visits(tab_id,last_seen_id,updated_at) SELECT NEW.id,COALESCE((SELECT last_seen_id FROM telesink_activity_visits WHERE user_id=NEW.user_id),(SELECT COALESCE(MAX(e.id),0) FROM events e JOIN sinks s ON s.id=e.sink_id JOIN sink_memberships m ON m.sink_id=s.id JOIN users u ON u.id=m.user_id WHERE m.user_id=NEW.user_id AND s.account_id=u.account_id)),now(); END;

INSERT INTO telesink_activity_tab_visits(tab_id,last_seen_id,updated_at)
SELECT t.id,COALESCE(v.last_seen_id,(SELECT COALESCE(MAX(e.id),0) FROM events e JOIN sinks s ON s.id=e.sink_id JOIN sink_memberships m ON m.sink_id=s.id JOIN users u ON u.id=m.user_id WHERE m.user_id=t.user_id AND s.account_id=u.account_id)),now()
FROM telesink_activity_tabs t LEFT JOIN telesink_activity_visits v ON v.user_id=t.user_id;
