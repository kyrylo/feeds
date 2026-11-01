# Preserved browser dependencies

These are the unchanged assets shipped by the Rails lockfile:

- `turbo.min.js`: turbo-rails 2.0.23, MIT license in `turbo-LICENSE`.
- `stimulus.min.js`, `stimulus-loading.js`: stimulus-rails 1.3.4, MIT
  license in `stimulus-LICENSE`.

The Rust build fingerprints and embeds these alongside the original application
CSS and JavaScript. Production and native asset builds do not need Ruby or Node.
Keep versions and licenses in sync when deliberately upgrading the frontend.
