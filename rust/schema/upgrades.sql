-- Native equivalents of db/migrate. Version numbers remain Rails-compatible.
-- Data backfills for accounts/search text run in schema.rs in this transaction.

-- version: 20260218070602
CREATE TABLE users (id bigserial PRIMARY KEY, email_address varchar NOT NULL, password_digest varchar NOT NULL, created_at timestamp(6) NOT NULL, updated_at timestamp(6) NOT NULL);
CREATE UNIQUE INDEX index_users_on_email_address ON users(email_address);

-- version: 20260218070603
CREATE TABLE sessions (id bigserial PRIMARY KEY, user_id bigint NOT NULL, ip_address varchar, user_agent varchar, created_at timestamp(6) NOT NULL, updated_at timestamp(6) NOT NULL, CONSTRAINT fk_rails_758836b4f0 FOREIGN KEY(user_id) REFERENCES users(id));
CREATE INDEX index_sessions_on_user_id ON sessions(user_id);

-- version: 20260219111001
CREATE TABLE sinks (id bigserial PRIMARY KEY, name varchar NOT NULL, created_at timestamp(6) NOT NULL, updated_at timestamp(6) NOT NULL);
CREATE INDEX index_sinks_on_name ON sinks(name);

-- version: 20260219111637
CREATE TABLE sink_memberships (id bigserial PRIMARY KEY, user_id bigint NOT NULL, sink_id bigint NOT NULL, created_at timestamp(6) NOT NULL, updated_at timestamp(6) NOT NULL, CONSTRAINT fk_rails_9d46e8dc99 FOREIGN KEY(user_id) REFERENCES users(id), CONSTRAINT fk_rails_9a1fa77e8d FOREIGN KEY(sink_id) REFERENCES sinks(id));
CREATE INDEX index_sink_memberships_on_user_id ON sink_memberships(user_id);
CREATE INDEX index_sink_memberships_on_sink_id ON sink_memberships(sink_id);
CREATE UNIQUE INDEX index_sink_memberships_on_user_id_and_sink_id ON sink_memberships(user_id,sink_id);

-- version: 20260221041229
ALTER TABLE sinks ADD COLUMN token varchar;
CREATE UNIQUE INDEX index_sinks_on_token ON sinks(token);

-- version: 20260221041249
CREATE TABLE events (id bigserial PRIMARY KEY, sink_id bigint NOT NULL, event_type varchar NOT NULL, text text, occurred_at timestamp(6) NOT NULL, payload jsonb NOT NULL DEFAULT '{}', created_at timestamp(6) NOT NULL, updated_at timestamp(6) NOT NULL, CONSTRAINT fk_rails_fb783782a6 FOREIGN KEY(sink_id) REFERENCES sinks(id));
CREATE INDEX index_events_on_sink_id ON events(sink_id);
CREATE INDEX index_events_on_event_type ON events(event_type);
CREATE INDEX index_events_on_sink_id_and_occurred_at ON events(sink_id,occurred_at DESC);
CREATE INDEX index_events_on_payload ON events USING gin(payload);

-- version: 20260224101644
CREATE TABLE columns (id bigserial PRIMARY KEY, sink_id bigint NOT NULL, name varchar NOT NULL, position integer NOT NULL DEFAULT 0, config jsonb, created_at timestamp(6) NOT NULL, updated_at timestamp(6) NOT NULL, CONSTRAINT fk_rails_0cdb2abfef FOREIGN KEY(sink_id) REFERENCES sinks(id));
CREATE INDEX index_columns_on_sink_id ON columns(sink_id);
CREATE INDEX index_columns_on_sink_id_and_position ON columns(sink_id,position);

-- version: 20260304063003
ALTER TABLE events ADD COLUMN emoji varchar(8);

-- version: 20260304090251
ALTER TABLE events ALTER COLUMN emoji SET DEFAULT '📌';
-- Fill old NULLs before enforcing the constraint; the old Rails order failed.
UPDATE events SET emoji='📌' WHERE emoji IS NULL;
ALTER TABLE events ALTER COLUMN emoji SET NOT NULL;

-- version: 20260313122729
ALTER TABLE events RENAME COLUMN payload TO properties;
ALTER INDEX index_events_on_payload RENAME TO index_events_on_properties;

-- version: 20260314100743
ALTER TABLE events ALTER COLUMN text SET NOT NULL;

-- version: 20260316083147
ALTER TABLE events ALTER COLUMN occurred_at DROP NOT NULL;

-- version: 20260316093300
ALTER TABLE events ADD COLUMN idempotency_key varchar;
CREATE UNIQUE INDEX index_events_on_sink_id_and_idempotency_key ON events(sink_id,idempotency_key);

-- version: 20260317055212
ALTER TABLE events ALTER COLUMN occurred_at SET NOT NULL;

-- version: 20260317124113
ALTER TABLE events ADD COLUMN sdk_name varchar, ADD COLUMN sdk_version varchar;

-- version: 20260320101852
CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;
CREATE INDEX index_events_on_text_trgm ON events USING gin(text gin_trgm_ops);

-- version: 20260322054000
ALTER TABLE columns ADD COLUMN has_events boolean NOT NULL DEFAULT false;

-- version: 20260327095016
ALTER TABLE sink_memberships ADD COLUMN has_unread_events boolean NOT NULL DEFAULT false;

-- version: 20260327133930
ALTER TABLE users ADD COLUMN current_sink_id bigint, ADD CONSTRAINT fk_rails_223f64897e FOREIGN KEY(current_sink_id) REFERENCES sinks(id) ON DELETE SET NULL;
CREATE INDEX index_users_on_current_sink_id ON users(current_sink_id);

-- version: 20260330111317
ALTER TABLE sink_memberships ADD COLUMN column_last_viewed_at jsonb NOT NULL DEFAULT '{}';

-- version: 20260405032127
ALTER TABLE users ADD COLUMN role integer NOT NULL DEFAULT 0;
UPDATE users SET role=1;

-- version: 20260406033731
CREATE TABLE accounts (id bigserial PRIMARY KEY, join_code varchar, created_at timestamp(6) NOT NULL, updated_at timestamp(6) NOT NULL);
CREATE UNIQUE INDEX index_accounts_on_join_code ON accounts(join_code);

-- version: 20260406090604
ALTER TABLE users ADD COLUMN account_id bigint, ADD CONSTRAINT fk_rails_61ac11da2b FOREIGN KEY(account_id) REFERENCES accounts(id);
CREATE INDEX index_users_on_account_id ON users(account_id);

-- version: 20260406090736
SELECT 1;

-- version: 20260406120336
ALTER TABLE users ADD COLUMN nickname varchar;
UPDATE users SET nickname=split_part(email_address,'@',1);

-- version: 20260414044811
ALTER TABLE sinks ADD COLUMN account_id bigint, ADD CONSTRAINT fk_rails_e7d49e6417 FOREIGN KEY(account_id) REFERENCES accounts(id);
CREATE INDEX index_sinks_on_account_id ON sinks(account_id);
UPDATE sinks SET account_id=COALESCE((SELECT users.account_id FROM users INNER JOIN sink_memberships ON sink_memberships.user_id=users.id WHERE sink_memberships.sink_id=sinks.id LIMIT 1),(SELECT id FROM accounts ORDER BY id ASC LIMIT 1)) WHERE account_id IS NULL;
ALTER TABLE sinks ALTER COLUMN account_id SET NOT NULL;

-- version: 20260417072357
CREATE TABLE folders (id bigserial PRIMARY KEY, name varchar, account_id bigint NOT NULL, created_at timestamp(6) NOT NULL, updated_at timestamp(6) NOT NULL, CONSTRAINT fk_rails_d2aa0af69f FOREIGN KEY(account_id) REFERENCES accounts(id));
CREATE INDEX index_folders_on_account_id ON folders(account_id);

-- version: 20260417075423
ALTER TABLE sinks ADD COLUMN folder_id bigint, ADD CONSTRAINT fk_rails_bccb0938c9 FOREIGN KEY(folder_id) REFERENCES folders(id);
CREATE INDEX index_sinks_on_folder_id ON sinks(folder_id);

-- version: 20260425044225
ALTER TABLE users ADD COLUMN current_folder_id bigint, ADD CONSTRAINT fk_rails_17b56651db FOREIGN KEY(current_folder_id) REFERENCES folders(id) ON DELETE SET NULL;
CREATE INDEX index_users_on_current_folder_id ON users(current_folder_id);

-- version: 20260625000000
ALTER TABLE sink_memberships ADD COLUMN unread_count integer NOT NULL DEFAULT 0, ADD COLUMN last_viewed_at timestamp(6);
UPDATE sink_memberships SET unread_count=1 WHERE has_unread_events=true AND unread_count=0;

-- version: 20260628000000
CREATE TABLE saved_views (id bigserial PRIMARY KEY, sink_id bigint NOT NULL, user_id bigint NOT NULL, name varchar NOT NULL, event_type varchar, event_date date, property_key varchar, property_op varchar, property_value varchar, created_at timestamp(6) NOT NULL, updated_at timestamp(6) NOT NULL, CONSTRAINT fk_rails_c8e04076d8 FOREIGN KEY(sink_id) REFERENCES sinks(id), CONSTRAINT fk_rails_ecb864ffd2 FOREIGN KEY(user_id) REFERENCES users(id));
CREATE INDEX index_saved_views_on_sink_id ON saved_views(sink_id);
CREATE INDEX index_saved_views_on_user_id ON saved_views(user_id);
CREATE UNIQUE INDEX index_saved_views_on_user_id_and_sink_id_and_name ON saved_views(user_id,sink_id,name);
CREATE INDEX index_saved_views_on_sink_id_and_user_id ON saved_views(sink_id,user_id);

-- version: 20260628010000
ALTER TABLE saved_views ADD COLUMN search_query varchar;

-- version: 20260629010000
ALTER TABLE events ADD COLUMN search_text text;
