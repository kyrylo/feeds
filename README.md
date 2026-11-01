# Feeds

A little log for your projects. Small, self-hosted, built with Rust and SQLite.
One executable, one database file, one owner.

Signups, payments, deployments, and whatever else you want to watch. Each feed
has its own private event URL. Events arrive live in a text log, with search,
expandable details, unread counts, and keyboard shortcuts. A little like IRC.

[Website](https://kyrylo.org/feeds/) · [Usage](docs/usage.md) · [Installation](docs/installation.md)

Previously called Telesink. Now a personal tool, free to self-host under MIT.
No subscription, frontend framework, database server, or background worker.

## Try it

Install Rust 1.96+, then:

```sh
cargo install --git https://github.com/kyrylo/feeds --locked --bin feeds
```

Create a session secret and set a password of at least 12 characters (Bash/Zsh):

```sh
export SECRET_KEY_BASE="$(openssl rand -hex 64)"
printf 'Owner password: '
IFS= read -r -s feeds_password
printf '\n'
export FEEDS_PASSWORD_HASH="$(printf '%s' "$feeds_password" | feeds hash-password)"
unset feeds_password
mkdir -p storage
feeds
```

Open [localhost:3000](http://localhost:3000), sign in, and create a feed in
**connect**. Copy its event URL and send some JSON:

```sh
curl 'http://localhost:3000/api/v1/feeds/YOUR_FEED_TOKEN/events' \
  -H 'Content-Type: application/json' \
  -d '{"event":"deployed","emoji":"🚀","text":"My project is live"}'
```

Only `text` is required. Keep the URL private. Add `properties`, `occurred_at`,
or an `Idempotency-Key` header when you need them. See the [usage guide](docs/usage.md).

Keep the **same `SECRET_KEY_BASE`** in your service environment across restarts.
Your password is saved in SQLite after the first start. Data lives in
`storage/feeds.sqlite3`; retain that directory. Feeds reads environment variables,
not `.env` files. All settings are in [.env.example](.env.example).

## Docker

[Build and run with Docker](docs/installation.md#docker). The runtime container
is non-root and contains no Ruby, Node, or shell. Mount a persistent data volume.

## Deploy with Kamal

[Deploy with Kamal](docs/installation.md#deploy-with-kamal) to one server with
HTTPS and a persistent SQLite volume, using `config/deploy.yml.example`.
`bin/deploy` tags each release with its full Git commit SHA. Your server address,
local deployment config, and secrets stay outside version control.

## Local demo

From a source checkout, run `bin/demo`. It uses Docker and Python 3 to start an
isolated demo at [localhost:4137](http://localhost:4137) with synthetic events.
Sign in with `feeds-demo-password`. Run `bin/demo stop` to stop it; its data is
kept for next time.

## Existing Telesink installations

Keep your database, data volume, and session secret. Feed IDs, event URL tokens,
passwords, events, and visit progress are preserved. The old
`/api/v1/sinks/TOKEN/events` routes and `TELESINK_*` settings remain supported;
`FEEDS_*` settings take precedence. The shared `POST /events` endpoint is retired;
use each feed’s event URL instead.

- [Change domains or migrate PostgreSQL](docs/sqlite-migration.md)
- [Backups, restore, and password recovery](docs/administration.md)

## Develop

```sh
export SECRET_KEY_BASE="$(openssl rand -hex 64)"
bin/setup --skip-server
bin/dev
bin/test
```

`bin/ci` runs formatting, strict Clippy, and the SQLite compatibility suite.
`bin/test-container feeds:local` verifies a built Docker image with disposable
data, including ingestion, backups, restart persistence, and password recovery.
The native app is in `rust/`. Retained Rails code and fixtures are compatibility
references, not runtime dependencies.

## License

[MIT](LICENSE). Bundled [Hotwire assets](rust/vendor/hotwired/README.md) retain
their own license notices.
