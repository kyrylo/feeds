# Administration

[Back to Feeds](../README.md)

## Reset a forgotten password

No SMTP is required. The command reads a new password from stdin, updates the
existing owner's credential, and signs out every session. With the same `DATABASE_URL` as your server:

```sh
(
  set +x
  printf 'New owner password: ' >&2
  IFS= read -r -s feeds_password || exit 1
  printf '\n' >&2
  builtin printf '%s' "$feeds_password" | feeds reset-password
)
```

For Docker, replace the final command after the pipe with:

```sh
docker run --rm -i -v feeds-data:/data feeds:local reset-password
```

Use the server's actual data volume. Recovery does not require a session secret
or bootstrap hash. It refuses missing databases and ambiguous imported multi-user
installations. Imported users can still sign in by email and use existing email
recovery if SMTP is configured.

## Backups and upgrades

```sh
feeds db check
feeds db backup /path/to/new-backup.sqlite3
```

Backups are consistent snapshots, including committed WAL data, and never
replace an existing file. Keep a copy off the application host. Do not copy only
the database file while it is running. Native SQLite upgrades run at startup.
See the [runtime and restore guide](../rust/README.md) and the
[PostgreSQL migration guide](sqlite-migration.md) for older installations.
