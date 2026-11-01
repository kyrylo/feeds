-- Existing saved search rows are retained; every event source gains a resource.
INSERT INTO telesink_activity_tabs(user_id,source_id,name,filters,position,created_at,updated_at)
SELECT m.user_id,s.id,s.name,'{}',COALESCE((SELECT MAX(position) FROM telesink_activity_tabs WHERE user_id=m.user_id),-1)+s.id,now(),now()
FROM sinks s JOIN sink_memberships m ON m.sink_id=s.id JOIN users u ON u.id=m.user_id AND u.account_id=s.account_id
WHERE NOT EXISTS(SELECT 1 FROM telesink_activity_tabs t WHERE t.user_id=m.user_id AND t.source_id=s.id);
