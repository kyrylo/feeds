# Run with bin/rails-reference runner rust/reference/export_contracts.rb.
# Only synthetic values are exported. Never run against an ordinary app database.
require "json"
require "fileutils"
require "active_support/testing/time_helpers"
include ActiveSupport::Testing::TimeHelpers

config = ActiveRecord::Base.connection_db_config.configuration_hash
abort "refusing non-reference database" unless config[:database] == "telesink_rust_reference" && config[:port].to_i == 55438

cases = [
  {}, { text: "Hello there" }, { text: "" }, { text: " \t\n" },
  { text: "\u00a0" }, { text: false }, { text: 0 },
  { text: "signed up", event: "", emoji: " " },
  { text: "hello", event: "user.signed_up", emoji: "👤", idempotency_key: "42", sdk: { name: "telesink-ruby", version: "1.3.0" } },
  { text: "Unicode: café 日本語 🚀", event: "request", properties: { user_id: 42, nil_value: nil, flag: false, nested: { array: [ 1, "two" ] } } },
  { text: "UTC", occurred_at: "2026-03-20T09:55:00.480Z" },
  { text: "offset", occurred_at: "2026-03-20T17:55:00.480+08:00" },
  { text: "fraction", occurred_at: "2026-03-20T09:55:00.123456Z" },
  { text: "invalid", occurred_at: "not-a-time" },
  { text: "blank key", idempotency_key: "   " }
]
now = Time.utc(2026, 10, 18, 0, 0, 0)
results = cases.map do |raw|
  event = Telesink::Event.new(raw.with_indifferent_access)
  if event.valid?
    stored = Event.new(event_type: event.event_type, text: event.text, properties: event.properties)
    {
      input: raw, valid: true,
      output: { event_type: stored.event_type, emoji: event.emoji, text: stored.text,
        properties: stored.properties, occurred_at: (event.occurred_at || now).iso8601(6),
        idempotency_key: event.idempotency_key, sdk_name: event.sdk_name, sdk_version: event.sdk_version },
      search_text: Event.search_text_for(stored)
    }
  else
    { input: raw, valid: false, errors: event.errors.full_messages }
  end
end

ActiveRecord::Base.transaction do
travel_to(now) do
  account = Account.create!
  user = account.users.create!(email_address: "rust-reference-#{SecureRandom.hex(6)}@example.test", nickname: "Reference", password: "synthetic-password", role: :owner)
  http = ActionDispatch::Integration::Session.new(Rails.application)
  http.post("/session", params: { email_address: user.email_address, password: "synthetic-password" })
  raise "reference login failed: #{http.response.status}" unless http.response.status == 302
  session_id = user.sessions.last.id
  cookie = http.cookies["session_id"]
  raise "reference cookie missing" unless cookie.present?
  output = {
    rails_version: Rails.version, schema_version: ActiveRecord::Base.connection_pool.migration_context.current_version,
    now: now.iso8601, events: results,
    session: { id: session_id, cookie: cookie, password_digest: user.password_digest, password: "synthetic-password" },
    tokens: {
      user_id: user.id, password_digest: user.password_digest,
      reset: user.password_reset_token, transfer: user.transfer_id,
      reset_purpose: User.token_definitions[:password_reset].full_purpose,
      reset_digest: User.generated_token_verifier.instance_variable_get(:@digest),
      transfer_digest: User.signed_id_verifier.instance_variable_get(:@digest)
    }
  }
  destination = Rails.root.join("rust/tests/fixtures/rails-contracts.json")
  FileUtils.mkdir_p(destination.dirname)
  File.write(destination, JSON.pretty_generate(output) + "\n")
  puts "Exported #{results.length} input cases and a real Rails HTTP session to #{destination.relative_path_from(Rails.root)}"
end
raise ActiveRecord::Rollback
end
