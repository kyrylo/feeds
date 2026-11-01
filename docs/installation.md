# Installation

[Back to Feeds](../README.md)

## Build from source


Install Rust 1.96, then build:

```sh
git clone https://github.com/kyrylo/feeds.git
cd feeds
cargo build --manifest-path rust/Cargo.toml --release --locked --bin feeds
```

Create a session secret and an owner password (Bash/Zsh):

```sh
export SECRET_KEY_BASE="$(openssl rand -hex 64)"
printf 'Owner password: '
IFS= read -r -s feeds_password
printf '\n'
export FEEDS_PASSWORD_HASH="$(printf '%s' "$feeds_password" | rust/target/release/feeds hash-password)"
unset feeds_password
rust/target/release/feeds
```

Use at least 12 characters (at most 72 bytes). Open
[localhost:3000](http://localhost:3000) and sign in with the password. No username
is needed. Open **connect**, create a feed, and copy its event URL or sample request.

The bootstrap hash creates the owner once, before the server accepts requests.
After that, credentials live in SQLite and the environment hash is ignored.
Change your password in **settings**, or use [password recovery](administration.md#reset-a-forgotten-password). An existing
installation keeps its account, password, source tokens, and events.

Keep `SECRET_KEY_BASE` in your service manager or secret store and supply the
**same value on every start**. Data lives in `storage/feeds.sqlite3`; keep the
directory between restarts. Feeds reads environment variables, not `.env`
files. See [.env.example](../.env.example) for configuration.

For local development, you can omit `FEEDS_PASSWORD_HASH` and create the one
owner through **set up Feeds** on the sign-in screen. Provision an owner before
exposing a new installation publicly. Setup closes once an owner exists.

## Docker

Clone the repository and build the image. Rust is built inside Docker:

```sh
git clone https://github.com/kyrylo/feeds.git
cd feeds
docker build -t feeds:local .
```

Create the two secrets (Bash/Zsh):

```sh
export SECRET_KEY_BASE="$(openssl rand -hex 64)"
printf 'Owner password: '
IFS= read -r -s feeds_password
printf '\n'
export FEEDS_PASSWORD_HASH="$(printf '%s' "$feeds_password" | docker run --rm -i feeds:local hash-password)"
unset feeds_password
```

Start the server:

```sh
docker run -d --name feeds --restart unless-stopped \
  -p 127.0.0.1:3000:3000 -v feeds-data:/data \
  -e SECRET_KEY_BASE -e FEEDS_PASSWORD_HASH \
  -e FEEDS_PUBLIC_URL=http://localhost:3000 feeds:local
```

Stop the native server first if it is using port 3000. The persistent volume
holds `/data/feeds.sqlite3`. For public access, put an HTTPS reverse proxy in
front and set `FEEDS_PUBLIC_URL` to the public origin. Keep the app port private.
The runtime container is non-root and contains no Ruby, Node, or shell.

## Deploy with Kamal

Like Owlytics, Feeds builds an image, lets Kamal provision HTTPS, and stores
SQLite on a persistent volume on one server. Install Kamal 2.12, point your
hostname to your server, and export your deployment settings:

```sh
export FEEDS_IMAGE=your-github-user/feeds
export FEEDS_SERVER=203.0.113.10
export FEEDS_HOST=events.example.com
export FEEDS_REGISTRY_USERNAME=your-github-user
# Supply KAMAL_REGISTRY_PASSWORD from your secret store (a registry push token).
# Also export SECRET_KEY_BASE and FEEDS_PASSWORD_HASH as above.
```

For a **new installation**:

```sh
cp config/deploy.yml.example config/deploy.yml
cp .kamal/secrets.example .kamal/secrets
bin/deploy setup
```

Open `https://YOUR_HOSTNAME` and sign in. Use `bin/deploy` for subsequent
releases, `kamal logs` for logs, and `kamal db_check` to verify SQLite.
The default registry is `ghcr.io`; use `FEEDS_REGISTRY` to change it and
`FEEDS_ARCH=arm64` for an ARM server.

`bin/deploy` requires a clean Git checkout and uses the full commit SHA as the
image tag. Kamal builds that committed source; keep the template’s default Git
build context so ignored local settings are excluded. Each installation supplies
its own host, image repository, and secrets through the ignored local files.
For existing installations, merge the template into your own configuration;
keep your original secret, host, and data volume. This image now listens on
port **3000**, so set `proxy.app_port: 3000` (or explicitly retain
`FEEDS_BIND=0.0.0.0:80` with `proxy.app_port: 80`).
