# Moving an existing installation

Feeds was previously called Telesink. Its native runtime uses SQLite. Existing
SQLite installations keep their accounts, password, feed IDs, URL tokens, events,
and visit progress when upgrading. The legacy sink event routes remain supported.

## Change your domain on the same server

A domain change does not require copying the database. Keep the existing Kamal
service identity, data volume, `DATABASE_URL`, and `SECRET_KEY_BASE`.

1. Create a verified snapshot with `feeds db backup /path/to/new-backup.sqlite3`.
   Keep the snapshot and the current deployment configuration for recovery.
2. Point the new hostname at the server. Keep the old hostname active while
   applications still send events to it.
3. Set `FEEDS_PUBLIC_URL` to the new HTTPS origin. In your private Kamal
   configuration, replace `proxy.host` with `proxy.hosts` containing both domains.
   Set `proxy.app_port: 3000` for the native image.
4. Deploy the tested image. Verify HTTPS and `/ready` on both domains, sign in,
   and check the feed list and historical events. Connect should display new
   event URLs; existing URLs keep the same tokens and continue accepting events.
5. Update event senders at your own pace. Keep the old host routed directly to
   Feeds until they have moved; some webhook clients do not follow redirects.
6. To retire the old address, set `proxy.host` to the new hostname only, remove
   `proxy.hosts`, and apply the proxy configuration. Verify that the new host
   serves Feeds and the old host no longer does. Event senders must use the new
   hostname after this step; their feed tokens stay the same.

Cookies are scoped to their host, so sign in again on the new domain. Keeping the
session secret preserves existing sessions on the old domain.

Do not replace an existing volume with `feeds-data` just to rename it: a new empty
volume creates a new installation. Storage names are internal and can remain as-is.
Do not remove old volumes or restore a stale backup over newly accepted events.

## Move SQLite to another server

Use a verified backup for a rehearsal on an isolated volume first. For the final
cutover, stop or pause all event senders and stop every database writer, then take
and verify a final snapshot. Transfer that snapshot and your runtime configuration
securely to the new host. Keep the original session secret and tokens. The runtime
container runs as UID/GID 65532; its database directory must be writable by that
user. Start Feeds, verify login, history, ingestion, and restart persistence, then
resume senders against the new host.

Once the new host accepts writes, the old database is stale. Do not resume it
without reconciling those newer events. See the [backup and restore guide](../rust/README.md#backups-and-restore).

## Import an older PostgreSQL installation

The ordinary runtime has no PostgreSQL driver. Build the separate `migration`
Docker target, rehearse against restored backups, and follow the full
[PostgreSQL cutover procedure](../rust/README.md#postgresql-cutover).
The importer refuses unresolved jobs, verifies imported rows and sequence floors,
and writes to a new SQLite path. Preserve the original source and backups until
you have verified the destination. A rehearsal import is never the final cutover.
