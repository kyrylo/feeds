# Native runtime replacement

Historical record of the first Rust port, initially verified with PostgreSQL and
the original interface. Production has since moved to native SQLite and the
per-source Feeds interface. See [the migration guide](sqlite-migration.md)
and [current runtime guide](../rust/README.md) for the deployed architecture.
The sections below describe the original compatibility work and its evidence.

## Objective and order

Replace the Rails backend with Rust to reduce steady-state RAM, image size, and
runtime dependencies. First preserve the current product and its data; apply the
unified-feed design only after that replacement is verified. The existing browser
HTML, CSS, JavaScript, Stimulus and Turbo behavior are part of phase-one scope.

Existing sink URLs and tokens remain valid. Events retain their sink IDs and
per-sink idempotency boundaries. A future unified feed queries authorized sources;
it does not merge sinks, broaden membership access, or require clients to reroute.

## Runtime design

- Rust HTTP service with bounded PostgreSQL connections and Tokio tasks.
- Keep the existing PostgreSQL application schema and stored identifiers.
- Persist ingestion before returning success, eliminating a separate ingestion
  worker. Preserve request/response contracts and handle pre-cutover queued jobs.
- Preserve existing credentials, bcrypt hashes, signed sessions and user access.
- Serve existing static browser assets. No Ruby/Rails fallback in the runtime.
- Implement required migration/management operations natively.
- Keep the original Rails application as an independent behavioral reference.

## Completion gates

- [x] Original tests inventoried; every observable contract covered or explicitly
      reconciled with the user-authorized runtime change.
- [x] All current route families, forms, redirects, validation and permissions.
- [x] Existing-schema reads/writes, IDs, tokens, JSON, timestamps and constraints.
- [x] Existing signed sessions, bcrypt passwords, reset and transfer links.
- [x] SDK/API ingestion, invalid requests, concurrent duplicates and durability.
- [x] Feed search, date/timezone/property filters, pagination and saved views.
- [x] Live updates, unread state, columns, folders and membership management.
- [x] Password email delivery and pending Rails job cutover.
- [x] Native startup/migrations and existing-installation upgrade/rollback checks.
- [x] Browser verification with original assets, including mobile layout.
- [x] Linux container runs without Ruby, Rails or build toolchains.
- [x] Comparable warmed Rails/native RAM measurements and Docker size evidence.
- [x] Documented native development, tests, configuration and deployment commands.

Use isolated synthetic PostgreSQL databases for verification. Never run test
schema loads against existing development, demo or production databases. Keep
measurement conditions, process counts and database memory accounting explicit.

## Initial environment

The initial checkout is clean `main`. PostgreSQL 16 tools and Rust are installed;
the default local PostgreSQL and Docker daemon are not running. The Rails lockfile
requires Ruby 4.0.5, available locally, and some gems need installation. The test
database helper owns only `tmp/rust-port/postgres` and port 55438.

## Verified implementation so far

The native HTTP router implements the existing ingestion URL, login/logout,
registration, invitation acceptance, existing password-reset links, account
recovery links, personal settings, invitations, member roles and sink access.
It now also implements the root redirect/welcome, sink CRUD, the feed, saved
views, event details, older-event loading, unread heartbeats, folders, event-type
management and retained column CRUD, ordering, pagination and view timestamps.
The native `/cable` endpoint now delivers live events and unread sidebar updates
through the original Action Cable/Turbo browser client.
Password-reset requests send email through a configured SMTP relay in the native
process. Demo mode preserves the existing shared user, browsing exceptions and
restrictions.
The existing CSS/Stimulus/Turbo assets are embedded at build time. No Ruby or
asset compiler is needed to build or run the native service.

The native suite passes all 64 scenarios: 11 unit/rendering/configuration checks, seven browser
authentication scenarios, five database/API ingestion scenarios, three independent
Rails authentication/ingestion comparisons, five account/team settings scenarios,
three feed comparisons, five sink/feed workflow scenarios, two folder/event-type
scenarios, three column scenarios, five live transport scenarios, four SMTP
scenarios, three demo scenarios, four schema-upgrade scenarios and four queue
handoff scenarios. These cover the implemented portions, not
full parity acceptance.
Strict all-target Clippy checks also pass. Signup, invitation acceptance and
recovery links retain their original destination; only ordinary login restores
the previously requested protected URL.

Five original Rack::Cors fixtures verify browser SDK preflights, allowed headers,
the two-hour cache, error responses and exclusion of account pages. Ten original
RemoteIp fixtures verify clients behind private/loopback proxies; an HTTP test
confirms separate login limits and stored session addresses for two clients of
the same proxy. Public direct peers cannot choose their identity through a
forwarded-address header.

The unchanged Rails app exported an actual login cookie, bcrypt hash, reset token
and transfer token. Rust accepts those outputs, checks expiry/purpose/signature,
and binds reset tokens to the stored password salt. In the other direction, Rails
accepted Rust-generated bcrypt, both link types and a native session cookie
through its original HTTP authentication flow. Changing the password invalidated
the native reset link in Rails. The reference verifier rolls back its synthetic
records after each run.

A local browser check against the native executable verified sign-in with a
Rails-created hash, return to a protected settings URL, nickname submission and
success banner, the owner team navigation, member links and logout. No browser
warnings/errors were reported. Screenshot: `tmp/rust-port/native-settings.jpg`.
This check found and fixed the `serde_json` arbitrary-precision representation
leaking into template numbers; regression coverage now checks role navigation,
year rendering and escaping.

A second browser check verified the feed on desktop and at 390×844: login with
the Rails-created hash, local timestamps, search, saving a view, expandable event
details, numeric property links, empty search results, mobile filters and source
switching. Scrolling loaded all 75 synthetic events from the initial 60, without
duplicates. No JavaScript warnings or errors appeared during these workflows.
Screenshots: `tmp/rust-port/native-feed-desktop.jpg` and
`tmp/rust-port/native-feed-mobile.jpg`. The browser later timed out during logout
and tab cleanup; logout was not verified in that check. The native server was
stopped and the synthetic account, sessions and records were removed directly
from the isolated database.

A third browser check verified native live delivery without navigation: new event
rows and unfiltered type counts, property-filtered delivery, exclusion of events
that did not match, and another sink's unread badge. The original Turbo client
reported no warnings or errors. Screenshot: `tmp/rust-port/native-live-feed.jpg`.
Browser automation stalled on the logout confirmation; the server was stopped
and all records for the synthetic browser account were removed directly.

Real WebSocket tests cover origin/session/signature rejection, cross-account
source rejection, filters, page-to-subscribe races, reconnect catch-up, overflow
refresh, unread updates, revocations and two native instances sharing PostgreSQL.
Terminating only a named synthetic LISTEN backend confirmed reconnection and
delivery recovery. Five independent Rails stream fixtures match native signatures
byte for byte. The relay uses one additional PostgreSQL connection per process;
512 sockets, eight subscriptions per socket, 16 KiB inbound messages and 512 queued
change IDs bound transport resources. Filtered counts and calendar totals update
on navigation, matching the original live behavior.

Four local SMTP scenarios verify multipart reset email, authentication, the
configured public URL, actual use of the delivered reset link, session revocation,
known/unknown-user notices, CSRF/rate limits, disabled/rejected delivery, required
STARTTLS without downgrade, and the two-connection bound. Configuration validation
is covered separately. The SMTP fixture never sends mail externally. Direct
delivery replaces the Rails mail job for new requests: success follows relay
acceptance; failure returns a retry banner. Requests have a twenty-second total
deadline and do not create a new queue or schema table. Already-queued Rails mail
jobs use the native cutover importer described below.

Three demo scenarios verify concurrent anonymous initialization, existing user
and source preservation, hidden account controls, saved-view creation, column
exceptions, role/source boundaries, local restriction redirects and API ingestion.
Unlike Rails' incomplete missing-user branch, native initialization creates the
required account with the demo user. Existing demo users are not changed. The
first anonymous visit uses one same-site redirect to establish its session.

The independent Rails exporter produced 198 feed query cases and 33 column cases.
The Rust database queries match their event order, normalized filters, counts,
newest timestamps and local calendar counts, including the March DST boundary
in New York. Empty-property values, numeric strings, wildcard characters and
ties in event timestamps are included. The older column query deliberately keeps
its text-only wildcard search and ID cursor rather than adopting feed semantics.
All test connections use UTC, as the native server does, even if PostgreSQL's
local default timezone differs.

The original Rails baseline completed 205 tests / 551 assertions with three
failures and four errors. Most expect an automatically created "all events"
column, which the current model no longer creates; another expects a successful
Turbo column response but receives a redirect. The original source is unchanged.
Evidence: `tmp/rust-port/rails-tests.log`. The native routes follow the current
implementation: creating a sink does not automatically create a column, and an
ordinary column GET redirects with 303 unless it requests a frame or stream.
New native scenarios cover those behaviors explicitly. The original tests and
source remain unchanged; do not claim the original baseline was green.

## Deliberate permission/validation corrections

- Sink access changes validate every requested sink against the acting user's
  account before changing memberships, within one transaction. The Rails update
  path accepted arbitrary sink IDs, while its form only listed account sinks.
  The native service rejects forged cross-account IDs and retains existing
  memberships on rejection. Keeping a membership does not reset unread state.
- A duplicate email change returns a validation banner and keeps the existing
  email rather than exposing an unhandled uniqueness error.
- Nested column and event-type routes require source membership in the same
  account, and column IDs must belong to the source in the URL. The original
  `SinkScoped` and `ColumnScoped` concerns used global finds. Forged IDs now fail
  without disclosing or changing another source. Failed reorders roll back all
  positions; concurrent column-view updates retain both JSON timestamp keys.
- Retained column cards open the current event permalink. The old cards targeted
  a column Turbo frame, but the current Rails event page only returns a detail
  frame, so the old card navigation could not complete.

## Native schema and queued-job handoff

The executable now creates empty databases and upgrades known Rails schema
versions without Ruby. Four integration scenarios compare the result against the
original Rails catalog and two independently generated historical schemas. They
verify concurrent startup, atomic rollback, original IDs/tokens/hashes/sessions,
unchanged current databases, original search backfills and native CLI operation.
Unknown or gapped histories, mismatched constraints, and indexes left invalid by
a failed concurrent build are rejected before serving.

Four queue scenarios use the real Solid Queue schema and job envelopes exported
by the unchanged Rails app. They verify events, password mail, obsolete Turbo
broadcasts, the exact finished-job cleanup, future schedules, stale claims,
unsupported/invalid jobs, SMTP failure/retry, and refusal of active/paused queues.
An injected queue acknowledgement failure proves an unkeyed event is not replayed:
the primary event and additive import receipt commit together before queue
acknowledgement. No framework job arguments are deleted or evaluated as code.
SMTP delivery has an unavoidable duplicate-email window if the process dies
after relay acceptance but before the primary commit. The cutover procedure in
`rust/README.md` describes that limit and preserving queue/receipt backups.

The old `/jobs` dashboard belongs to the retired Solid Queue runtime. Native
operation has no asynchronous ingestion queue; `queue inspect` reports the
remaining legacy work during cutover. The old application remains available for
inspecting unsupported framework jobs.

## Container and resource evidence

On 2026-10-18, the native and unchanged Rails images ran the same synthetic
workload on Linux/amd64 in OrbStack (Docker 27.4.1). One PostgreSQL 16.15 server,
with `shared_buffers=32MB`, held separate application databases. Each application
started with one owner, one source, one retained column and 1,000 events. A
signed-in client made 100 page reads and 52 ingestion requests, including a
duplicate, while a real Action Cable subscription verified live delivery. Both
databases ended with 1,051 events before the rollback check.

After five seconds settling, ten samples one second apart recorded Docker's
container memory usage minus inactive file cache. These are application cgroup
figures, not sums of process RSS and not whole-installation memory. PostgreSQL
is excluded; its shared test server used about 101 MiB after the Rails workload.
This measures a warmed small workload, not throughput capacity or a promise for
every event size and traffic level.

| Measurement | Original Rails | Rust |
| --- | ---: | ---: |
| Uncompressed image bytes | 468,729,610 | 35,256,382 |
| Median warmed app memory | 448.77 MiB | 18.38 MiB |
| Observed memory range | 447.37–450.54 MiB | 18.38–18.93 MiB |
| Application processes | 6 | 1 |

That is about 96% less application memory and a 92% smaller image in this run.
Rails included Thruster, Puma, Solid Queue supervisor, dispatcher, worker and
scheduler, with Solid Cable enabled. Native included HTTP, live updates,
ingestion and its readiness check, running as UID 65532 with a read-only root,
all capabilities dropped and no new privileges. Its executable is 11,547,768
bytes. Image inspection confirmed absence of Ruby, Node, shells, PostgreSQL
clients, Cargo and build sources; the application/browser license notices remain.

The Rails image was built from an archive of the unchanged reference commit.
Image IDs: Rails `65eab20f2ffe4bb68ab9fad398458b1f3f4d7f2eec7964ae17d60617a3a8f956`;
native `56ad20b6150f1303fed81c4633a16c9595cfa94a424c328dc8aa8c558c7311e0`.
The reusable workload is `rust/reference/container_check.py`; raw samples,
process lists, image metadata, logs and filesystem checks are in
`tmp/rust-port/container-{rails,native,files}.json` and the matching logs.

Native SIGTERM shutdown exited successfully; restart retained the signed-in
session and all events. The unchanged production Rails image then started against
the native-created primary schema, accepted the native cookie, read the retained
column and wrote an event through its original API and queue worker. Restarting
native read that event and deduplicated its original key. Migration history and
legacy column options were retained. Evidence: `container-rollback.json`.

A final browser check against the container signed in, rendered the feed, opened
the retained column editor, saved a text filter, displayed the matching
Rails-written event and followed its event link. No browser warnings or errors
were reported. Screenshot: `tmp/rust-port/native-column.jpg`.

The root Dockerfile now builds the verified recipe; the original recipe is
retained at `rust/reference/Dockerfile.rails`. Native development and CI wrappers
use the locked Cargo dependencies. CI configuration is updated; a hosted CI run
will occur after the changes are pushed. The product simplification remains a
separate next step, with existing sources preserved as its compatibility boundary.

Final default-entrypoint verification passed: `bin/ci` ran formatting, strict
all-target Clippy and all 64 tests with no failures or ignored tests. `bin/setup
--skip-server` created a new synthetic database; repeated preparation made no
changes and `db check` accepted it. The root Docker build produced the identical
measured image ID and its CLI smoke check passed. Deployment and workflow YAML
syntax checks passed. The tracked Kamal example now uses native settings; private
deployment files were not changed. Logs are `tmp/rust-port/default-{ci,setup}.log`
and `tmp/rust-port/docker-default-build.log`.
