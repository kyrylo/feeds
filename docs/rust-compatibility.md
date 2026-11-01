# Compatibility inventory

The independent reference is commit
`24322b336460cd4834dafec07aee410b4fdfd414`. Its 33 original test files contain
205 tests. They remain unchanged. The native scenarios combine related HTTP and
database assertions; their test count is not a one-to-one conversion of Rails
tests. Additional original-model exports supply 198 feed cases, 33 column cases,
authentication wire fixtures, stream signatures, CORS responses, migration schemas
and job data.

| Original test family | Rails tests | Native evidence |
| --- | ---: | --- |
| Accounts and join-code rotation | 6 | `browser_auth`, `settings` |
| SDK event defaults, API ingestion, processing jobs | 15 | `events` unit tests, `ingestion`, `rails_contracts`, `queue` |
| Event model validation, column/feed queries | 14 | `ingestion`, `feed`, `columns`, original query fixtures |
| Event detail controller | 3 | `sinks`, `feed` membership tests |
| Folder controller | 14 | `management` |
| Invitations and registrations | 13 | `browser_auth`, `settings` |
| Sessions and transfer sign-in | 7 | `browser_auth`, `rails_contracts`, reverse Rails HTTP verifier |
| Password-reset controller | 7 | `browser_auth`, `mail`, `queue`, original reset-token fixtures |
| User model and email model checks | 16 | `browser_auth`, `settings`, `rails_contracts` |
| Personal nickname/password settings and settings redirect | 8 | `settings` |
| Invitation page | 3 | `settings` |
| Member list, roles, source grants and recovery links | 30 | `settings`, `browser_auth` |
| Source controller and `/new` redirect | 24 | `sinks`, `demo` |
| Source model | 4 | `sinks`, `columns`; obsolete default-column expectation reconciled below |
| Column model, CRUD, ordering and view timestamps | 26 | `columns`, `feed`, `live`, original column query fixtures |
| Membership unread/view behavior | 5 | `ingestion`, `sinks`, `columns`, `live` |
| Event-type management | 9 | `management` |
| Demo saved-view creation | 1 | `demo`, `sinks` |
| **Total** | **205** | **64 native scenarios, plus independent fixture comparisons** |

The native router also covers feed pagination, saved-view deletion, `/cable`,
readiness, SMTP, demo restrictions and startup/cutover operations that lack a
dedicated original controller test. `docs/rust-port.md` records browser and live
transport evidence separately.

## Reconciled behavior

- The unchanged Rails baseline has three failures and four errors, mostly from
  tests assuming creation of a default column. The current `Sink` model has no
  such callback. Native source creation follows the current model and preserves
  any existing columns. An ordinary column GET follows the current redirect;
  frame/stream requests render retained column cards. Old card links now target
  the working event permalink.
- Current user-facing source ordering is tested through the HTTP pages. The
  Rails model's unused relation/helper methods are represented only where they
  affect the product; no Active Record API is exposed by Rust.
- Forged cross-account membership updates and nested column/event-type access
  are rejected. Column-view requests for an inaccessible source return 404
  rather than the old controller's successful no-op. These do not alter existing
  source grants. Failed writes leave memberships, unread counts and ordering
  intact.
- Event ingestion commits before returning 201, replacing the worker. Database
  failure returns 503. Oversize idempotency keys that Rails would acknowledge
  before its later model validation failure are not acknowledged by Rust.
  Valid payloads keep the existing URL, token, defaults, property encoding and
  per-source deduplication. Pre-cutover jobs use the native importer.
- New reset email is sent directly and success follows SMTP acceptance. Disabled
  or failed SMTP returns a retry notice. Old reset/transfer links and password
  hashes remain valid with the original `SECRET_KEY_BASE`.
- The Solid Queue `/jobs` framework dashboard is retired with the queue runtime.
  `queue inspect` and the retained Rails reference support the one-time handoff.
  Unknown job classes are retained, not silently discarded.
- Demo initialization creates the required account along with a missing shared
  demo user. Existing demo users, credentials and source grants are preserved.
- Reverse-proxy client IPs follow Rails' default private/loopback proxy ranges,
  preserving separate login/reset limits. Public direct peers cannot override
  their socket address with forwarded headers. Conflicting proxy identity
  headers fail before session creation.

Schema verification compares required columns, constraints, indexes (including
their usable state) and `pg_trgm` against the original database catalog. It
permits additional application tables, including the optional import receipt
table, without rewriting them. Historical upgrades use original migration IDs;
the unified-feed product change has not been applied.
