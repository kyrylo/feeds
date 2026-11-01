# Use Feeds

[Back to Feeds](../README.md)



- Create, rename, and delete feeds from **connect**. Each feed has a private event
  URL. Renaming keeps the URL; replacing it revokes the old URL immediately.
- Home opens the first feed. Feeds have independent unread counts and can be
  reordered. Deleting a feed permanently deletes its events after confirmation.
- Search event text and data within a feed. Clear the search to see all its events.
- Event titles sit before each message, with a consistent color for each type.
  Emojis make the log easier to scan. Expand an event for properties, its timestamp,
  and a permalink.
- The log runs from oldest to newest, opening at the bottom. Live events follow
  while you are at the bottom; scrolling up keeps your place. Load earlier events
  from the top of the log.
- Events update automatically. Red markers show what arrived since your last visit.
- Press `/` to search, `j` / `k` to move through events, `Enter` to expand, and
  `?` for keyboard help, also available in the header. The feed list collapses on
  small screens.
- Light and dark appearance follow your system setting automatically.

## Send an event

Create a feed in **connect**, then copy its URL. The URL identifies and
authenticates that feed; no shared API key or `feed` field is needed.

```sh
curl 'https://events.example.com/api/v1/feeds/YOUR_FEED_TOKEN/events' \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: signup-123' \
  -d '{
    "event": "user_signed_up",
    "emoji": "👋",
    "text": "Maya signed up",
    "properties": { "plan": "Pro" }
  }'
```

Only `text` is required. `event`, `emoji`, `properties`, and `occurred_at`
(ISO 8601) are optional. Retries with the same idempotency key are saved once
per feed, including after renaming or replacing its URL. A `201` means the
event is committed to SQLite. Keep the URL private.

Existing `/api/v1/sinks/TOKEN/events` URLs continue to accept events for the same
feed. Replacing a feed URL revokes both forms of the old URL. The shared
`POST /events` endpoint and `/connect/key` have been removed; applications that
used them must switch to the corresponding feed URL.

`TELESINK_*` runtime settings remain accepted for existing installations.
`FEEDS_*` settings take precedence. Native startup keeps using
`storage/telesink.sqlite3` if that file exists; otherwise it creates
`storage/feeds.sqlite3`. When upgrading Docker or Kamal, retain your existing
`DATABASE_URL` and volume name. Existing events, feed resource IDs, URL tokens,
and visit progress are preserved. Stored saved-search definitions remain in
SQLite, but the interface now shows one feed per event source.
