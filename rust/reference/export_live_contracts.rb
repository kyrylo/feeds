require "json"
config = ActiveRecord::Base.connection_db_config.configuration_hash
abort "refusing non-reference database" unless config[:database] == "telesink_rust_reference" && config[:port].to_i == 55438
ActiveRecord::Base.transaction do
  account = Account.create!
  sink = account.sinks.create!(name: "Live reference")
  streams = [
    ["user_123_sinks"],
    [sink, "events", Event.feed_stream_key(nil)],
    [sink, "events", Event.feed_stream_key("payment_failed", date: Date.new(2026, 10, 18), property_key: "amount", property_op: "gt", property_value: "10")],
    [sink, "event_type_counts"],
    ['telesink/v1:{"kind":"sidebar","user":123}']
  ].map do |streamables|
    token = Turbo::StreamsChannel.signed_stream_name(streamables)
    { name: Turbo::StreamsChannel.verified_stream_name(token), token: token }
  end
  File.write(Rails.root.join("rust/tests/fixtures/live-contracts.json"), JSON.pretty_generate(streams) + "\n")
  puts "Exported #{streams.size} signed stream contracts from original Rails."
  raise ActiveRecord::Rollback
end
