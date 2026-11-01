# Native Feeds

Feeds runs as one Rust process with a local SQLite file. Deploy it with Kamal;
Ruby, Node, PostgreSQL, Redis and a separate job worker are not runtime requirements.
Assets, templates and schema definitions are embedded. The retained Rails source
is an independent compatibility reference, not part of the runtime image.

## Build and run

```sh
export SECRET_KEY_BASE="$(openssl rand -hex 64)"
bin/setup --skip-server
bin/dev
```

Keep the **original** `SECRET_KEY_BASE` when migrating an existing installation:
sessions, reset links and signed browser streams depend on it. Never replace it
with the example generation command during a migration.

`DATABASE_URL` defaults to `sqlite://storage/feeds.sqlite3` (or the existing
`storage/telesink.sqlite3` file when upgrading). `bin/dev` listens
on localhost:3000; the executable also defaults to localhost:3000. Set `FEEDS_BIND`
to override this. The container listens on port 3000 and defaults to
`sqlite:///data/feeds.sqlite3`. The database directory must already exist and
be writable by the process, including its WAL and shared-memory files.

The runtime uses five database connections by default (override with
`FEEDS_DATABASE_CONNECTIONS`), two async workers and at most two blocking
workers for password hashing. SQLite is bundled at 3.51.3, including the
[WAL-reset fix](https://www.sqlite.org/wal.html#walreset). Connections enable
foreign keys, WAL, full commit synchronization and a five-second busy timeout.
Use a local filesystem; sharing the database over NFS is unsupported.

SQLite also needs a writable temporary directory for large sorts and index
creation. If you run the container with `--read-only`, provide `/tmp`, for
example `--tmpfs /tmp:rw,noexec,nosuid,size=64m,mode=1777`. The persistent `/data`
mount must remain writable as well.

## Search scope

Feeds exposes one text search within each feed across event names, messages,
and properties. Each feed owns an ingestion URL. Feed creation, renaming, URL
rotation, and deletion require an owner session and CSRF protection. Deletion
removes the source and its events in one transaction; URL authentication is
rechecked under the same writer lock as deletion and rotation.

Upgrade `2026103001` creates missing feed resources without changing existing
events, resource IDs, credentials, or visit watermarks. Existing saved searches
remain stored, but the interface selects one resource per source (preferring an
unfiltered resource). Search parameters cannot switch a `/feeds/:id` resource
to another feed. The shared key endpoint has been removed; use each feed's URL.

## Database commands

```sh
feeds db prepare
feeds db check
feeds db backup /data/backups/feeds-2026-10-18.sqlite3
feeds serve
```

`prepare` creates and initializes a missing SQLite file; `serve` prepares it before
accepting traffic. Unrelated files, unknown schema versions, missing constraints,
dangling foreign keys and failed integrity checks are rejected. Existing records,
IDs and Rails metadata are preserved. `check` opens the database read-only and
never bootstraps it. These commands do not require a session secret.

SQLite may need to create WAL coordination files in the directory even for a
read-only check. For a closed, checkpointed snapshot on read-only media, append
`?immutable=true` to its URL. Never use this option on a live database: it ignores
the WAL and assumes the file cannot change.

`FEEDS_ENV` (fallback `RAILS_ENV`, default `production`) labels only a newly
initialized database. The server rejects PostgreSQL URLs and legacy
`POSTGRES_*`/`SOLID_QUEUE_IN_PUMA` settings so an old deployment configuration
cannot silently create a new empty SQLite database.

`/up` checks HTTP. `/ready` checks the database and live relay, and is used by
`feeds healthcheck` and the Kamal example. Shutdown handles SIGTERM.

## Kamal and persistent storage

```sh
docker build -t feeds-rust .
docker run --rm -p 127.0.0.1:3000:3000 --env-file /path/to/runtime.env \
  -v feeds-data:/data feeds-rust
```

Use `config/deploy.yml.example` as the deployment template. The named volume
`feeds-data:/data` persists across releases and is initialized with ownership
for UID/GID 65532. A manually created bind mount must have the same writable
ownership. The container is non-root and has no shell. Store the secret in Kamal's
secret configuration, and set the public origin and SMTP settings below.

All web containers serving one installation must access the **same local file on
one host**. Kamal's brief old/new container overlap is supported. Replicas on
separate hosts with independent files are not supported. Retain the data volume
when removing old application containers or images.

Kamal normally builds a Git clone. Commit the intended source before deployment,
or explicitly configure `builder.context: .` when intentionally deploying the
local working tree. Otherwise untracked Rust files and local edits are omitted.
The Docker context excludes local credentials, databases and build output.

## Backups and restore

`db backup PATH` uses SQLite `VACUUM INTO` to capture a consistent live snapshot,
including committed WAL pages. It creates a private staging file, verifies the
schema and integrity, synchronizes it to disk, then publishes a new filename.
It refuses to overwrite any existing file. Create the destination directory first.
Keep a copy off the application host, with the matching secret stored securely.
Do **not** copy only the live `.sqlite3` file while writers are running; recent
commits can still be in its WAL. See [SQLite backup guidance](https://www.sqlite.org/backup.html).

To restore, stop every web and maintenance writer. Preserve the current database
and its `-wal`/`-shm` siblings together for investigation. Copy a verified backup
to a new pathname on the data volume, give it the runtime's ownership, run
`db check` against that path, and point `DATABASE_URL` at it before restarting.
Never pair a restored main file with WAL files from another database. Test restore
on an isolated volume before relying on a backup. Events received after a backup
are absent from that backup; do not overwrite a newer live database with it.

## PostgreSQL cutover

See the [migration guide](../docs/sqlite-migration.md) for domain and host changes.
Do not treat local tests as proof that production has been migrated.

Build the separate one-time migration image:

```sh
docker build --target migration -t feeds-migration .
```

The ordinary image does not include the PostgreSQL driver. The importer reads
`POSTGRES_DATABASE_URL` and, for a Rails installation, `QUEUE_DATABASE_URL`.
It requires `--source-stopped --output /data/NEW.sqlite3`. Use `--no-legacy-queue`
only if that installation's legacy queue has already been retired.

For an isolated restore rehearsal, append `--rehearsal`. This reports unfinished
queue jobs without replaying or discarding them, and labels the result as a
rehearsal snapshot. It does not waive the queue checks for the eventual cutover;
never deploy a rehearsal file as the final database.

1. Inventory the actual deployment, database versions, volumes and all writers.
   Back up primary and framework databases, the current image and deployment
   configuration. Restore the backups in isolation and rehearse the conversion.
2. Stop incoming writes. Let Rails finish accepted jobs, including scheduled mail
   and events, then stop every web, queue worker and scheduler. Resolve failed,
   unsupported or future jobs explicitly. The importer refuses unfinished jobs or
   worker heartbeats newer than 60 seconds. A flag does not stop these processes.
3. Take the final backups. Run the importer against the stopped source, writing a
   **new** SQLite path. Save its JSON report. It uses a read-only repeatable-read
   source snapshot, compares every row's normalized value and count, preserves
   sequence high-water marks, and verifies foreign keys and integrity. Unknown
   tables, columns, schema versions and unsupported types cause refusal.
4. Preserve the host, API routes, source tokens and `SECRET_KEY_BASE`. Mount the
   imported database volume and update the private Kamal configuration to SQLite.
   Remove the old PostgreSQL environment variables and framework worker settings.
   Keep PostgreSQL stopped and available for recovery during verification.
5. Deploy and verify login, existing source URLs, idempotent retries, historical
   events, filtering, permissions, live updates and a new write followed by restart.
   If maintenance mode was enabled, explicitly run `kamal app live` and verify
   a normal public page: deployment can retain maintenance state even while its
   readiness endpoint responds successfully.
   Make and restore a SQLite backup. Only then retire the owned PostgreSQL
   container/data and obsolete images, retaining a verified off-host backup.

Before SQLite accepts writes, rollback can return to the stopped PostgreSQL
snapshot and old image. **After SQLite accepts writes, that snapshot is stale.**
Stop writes and reconcile the newer SQLite data before any reverse migration;
there is no automatic SQLite-to-PostgreSQL converter. A normal application-image
rollback can reuse SQLite only when that older image understands its schema.

The optional `postgres-import` feature retains tested legacy `queue inspect` and
`queue drain --rails-stopped` maintenance commands for recovery against a copied
SQLite primary. They are not in the ordinary image and do not replace resolving
the queue before the standard conversion. Event insertion and the handoff receipt
commit together; acknowledgement retries do not duplicate an imported event.
SMTP acceptance and database commits cannot be atomic, so retrying after an
uncertain mail delivery can send a duplicate reset email.

## Password reset email

Configure the SMTP relay and the public origin used in reset links:

```sh
FEEDS_PUBLIC_URL=https://events.example.com
SMTP_HOST=smtp.example.com
SMTP_FROM='Feeds <reset@example.com>'
SMTP_USERNAME=your-smtp-user
SMTP_PASSWORD=your-smtp-password
```

`SMTP_TLS` defaults to `starttls` (required TLS, port 587). Use `tls` for implicit
TLS (port 465), or explicitly `none` for an unencrypted internal relay (port 25).
`SMTP_PORT` overrides the mode's port. Authentication is optional; omit both
username and password for a relay without authentication. Invalid or partial
configuration fails startup. Omitting all SMTP settings disables reset email;
already-issued reset links still work.

The process sends multipart text/HTML email directly, using the original subject,
wording and Rails-compatible 15-minute token. The public URL must be an http(s)
origin; request Host/forwarded headers cannot change emailed links. SMTP uses
Rustls certificate validation, up to two concurrent connections, five-second I/O
timeouts and a twenty-second total deadline. It has no idle SMTP pool or worker.
Successful requests retain the generic known/unknown-email notice. A disabled,
busy or failed relay returns a retry banner; no request is left in an in-memory
queue. A crash or uncertain SMTP outcome can require the user to request another
email. Existing queued Rails mail jobs use the cutover procedure above.

## Compatibility and tests

Existing ingestion URLs, source-scoped idempotency keys, memberships, passwords,
Rails session cookies and signed links remain compatible. JSON numbers retain
arbitrary precision. A fresh installation allows creation of one account, then closes registration.
Invitations and team-management routes are retired. Imported accounts and their
memberships remain intact unless an explicit consolidation is requested. The native browser uses an embedded terminal-style interface. Retained compatibility routes use the
original Action Cable/Turbo protocol. A bounded SQLite change log carries committed event
and user notices between overlapping processes; gaps trigger a refresh. Access
is rechecked on subscriptions and deliveries. No separate cable database is used.

The API retains the original browser SDK CORS policy. Browser account routes do
not allow cross-origin access. Behind a private proxy, rate limits and session
addresses use the rightmost untrusted forwarded address; the edge proxy must set
forwarding headers correctly. Conflicting `Client-IP` headers are rejected.

```sh
bin/test
bin/ci
```

The ordinary suite creates synthetic SQLite files under
`tmp/sqlite-migration/tests` and needs no database server. It includes the
independent Rails fixtures for 198 feed queries and 33 legacy column cases,
wire authentication contracts, permission checks, ingestion retries, live
updates, SMTP, backup restoration and abrupt process termination.

The importer and retained legacy queue tests additionally need the explicitly
isolated PostgreSQL helper, never a production URL:

```sh
bin/rust-test-db start
TELESINK_TEST_DATABASE_URL=postgresql://telesink@127.0.0.1:55438/telesink_rust_test \
  cargo test --manifest-path rust/Cargo.toml --locked --features postgres-import \
  --test import_postgres --test queue
bin/rust-test-db stop
```

The historical Rails exporters and reference Dockerfile remain under
`rust/reference`. Their fixtures contain synthetic data. See
[the Rust rewrite record](../docs/rust-port.md) for earlier reference checks;
those earlier PostgreSQL measurements are not measurements of this SQLite build.

## Live feed behavior

The terminal interface shows one resource per event source at `/feeds/:id`.
Connect creates and manages those feeds and their private event URLs. The shared
`POST /events` endpoint has been retired. See the [usage guide](../docs/usage.md).

Activity polls for new event IDs while the page is visible. Pagination and live
updates enforce the same source and text search. History pages use a fixed
snapshot boundary; live pages advance an ID cursor in bounded batches, including
events with older timestamps. Rows display in ascending timestamp and ID order.
The log opens at the bottom and follows arrivals only while the reader is there;
earlier pages load above without moving the visible rows.

Each feed keeps its own unread progress. Last-visit updates require CSRF
protection and a signed snapshot belonging to the logged-in owner. Acknowledging
one feed cannot mark unseen events in another feed as read.

`tests/activity.rs` covers the current routes. Legacy sink, folder, column, and
socket regression tests use a test-only reference router; they preserve old
query and rendering contracts without exposing retired management screens.

## Owner provisioning and terminal interface

For new deployments, generate `FEEDS_PASSWORD_HASH` with `feeds hash-password`
(reading the password from stdin). On an empty installation the server creates
one owner before accepting connections. `FEEDS_OWNER_EMAIL` optionally sets a
contact address; the default is `owner@localhost`. Once an account exists, stored
credentials take precedence and the bootstrap variables are ignored. Single-owner
installations sign in with only a password. Imported multi-user installations
retain email-based login and are never consolidated automatically.

`feeds reset-password` reads a 12-character minimum, 72-byte maximum password
from stdin. It requires an existing compatible database with exactly one user,
changes that user's password and removes their sessions in one transaction. It
works without SMTP, `SECRET_KEY_BASE`, or `FEEDS_PASSWORD_HASH`. Existing login
throttles expire normally (within three minutes); restarting the process also
clears those in-memory throttles. See the [administration guide](../docs/administration.md) for hidden-prompt examples.

The terminal interface presents one list of feeds, with
monospace event rows and local timestamps. Light and dark appearance follow the
system setting through CSS `color-scheme` and `light-dark()`. A single header holds
keyboard help and the mobile feed picker; new-event markers use red text and rules.
The log updates automatically without a pause control. Keyboard navigation is
implemented in the embedded JavaScript. Saved feed
filters, unread positions, API URLs, and event data retain their existing schema.
The runtime now defaults to port 3000 both natively and in Docker. Existing Kamal
configs must set `proxy.app_port: 3000` or explicitly override the container bind
address back to port 80.
