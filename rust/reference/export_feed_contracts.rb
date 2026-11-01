# Run only against the dedicated reference database, never alongside Rails tests.
require "json"
require "fileutils"
config = ActiveRecord::Base.connection_db_config.configuration_hash
abort "refusing non-reference database" unless config[:database] == "telesink_rust_reference" && config[:port].to_i == 55438
FileUtils.mkdir_p(Rails.root.join("rust/data"))
File.write(Rails.root.join("rust/data/event-labels.json"), JSON.pretty_generate(Event::EVENT_TYPE_LABELS) + "\n")
File.write(Rails.root.join("rust/data/time-zones.json"), JSON.pretty_generate(ActiveSupport::TimeZone::MAPPING) + "\n")

ActiveRecord::Base.transaction do
  account = Account.create!
  sink = account.sinks.create!(name: "Feed reference")
  base = Time.utc(2026, 3, 8, 4, 0, 0)
  inputs = 75.times.map do |i|
    { event_type: %w[payment_succeeded deploy_finished HTTP][i % 3], text: "sample #{i}",
      properties: { amount: [i, i.to_s, "unavailable"][i % 3], plan: i.even? ? "pro" : "free" },
      occurred_at: (base + (i / 2) * 1800).iso8601(6) }
  end
  inputs += [
    { event_type: "custom_name", text: "100% literal_under_score \\ slash", properties: { empty: "", flag: false, nested: { x: 1 }, nil: nil }, occurred_at: "2026-03-08T06:59:59.999999Z" },
    { event_type: "custom_name", text: "day boundary", properties: { amount: " -12.5 " }, occurred_at: "2026-03-09T03:59:59.999999Z" },
    { event_type: "custom_name", text: "next day", properties: { amount: "1.5e2" }, occurred_at: "2026-03-09T04:00:00Z" },
    { event_type: "café 🚀", text: "Unicode", properties: { amount: ".5" }, occurred_at: "2026-03-08T06:00:00Z" }
  ]
  records = inputs.map { |input| sink.events.create!(input) }
  indexes = records.map.with_index { |event, i| [event.id, i] }.to_h
  cases = [
    {}, { event_type: " payment_succeeded " }, { event_type: "unknown" },
    { q: "sample 7" }, { q: "%" }, { q: "_" }, { q: "\\" }, { q: "café" },
    { property_key: "plan", property_value: "pro" }, { property_key: "empty", property_value: "" },
    { property_key: "flag", property_value: "false" }, { property_key: "nil", property_op: "exists" },
    { property_key: "amount", property_op: "gt", property_value: "20" },
    { property_key: "amount", property_op: "lt", property_value: "1" },
    { property_key: "amount", property_op: "lt", property_value: "NaN" },
    { property_key: "amount", property_op: "bad", property_value: "1" },
    { property_key: "empty" }, { property_key: "empty", property_value: nil },
    { date: "2026-03-08" }, { date: "2026-03-09" }, { date: "invalid" },
    { date: "2026-03-08", event_type: "HTTP", q: "sample", property_key: "plan", property_value: "pro" }
  ]
  queries = []
  %w[UTC America/New_York Asia/Manila].each do |zone|
    cases.each do |raw|
      filters = Event.normalize_feed_filters(raw)
      args = filters.except(:event_date).merge(date: filters[:event_date], time_zone: zone)
      [nil, records[40].id, records[2].id].each do |anchor|
        events = Event.feed_batch(sink, **args, before_id: anchor)
        query = { params: raw, zone: zone, before_index: anchor && indexes[anchor],
          normalized: filters, indexes: events.map { |event| indexes.fetch(event.id) } }
        if anchor.nil?
          query[:counts] = Event.feed_scope(sink, **args.except(:event_type)).group(:event_type).order(:event_type).count.to_a
          calendar_scope = Event.feed_scope(sink, **args.except(:date))
          query[:newest] = calendar_scope.maximum(:occurred_at)&.iso8601(6)
          query[:dates] = calendar_scope.where(occurred_at: Event.month_range_for(Date.new(2026, 3, 1), time_zone: zone))
            .group(Arel.sql(Event.local_date_sql(time_zone: zone))).order(Arel.sql(Event.local_date_sql(time_zone: zone))).count.map { |date, count| [date.iso8601, count] }
        end
        queries << query
      end
    end
  end
  helper = Object.new.extend(EventsHelper)
  labels = ["HTTP", "custom_name", "payment_succeeded", "render", "MiXeD_Name", "cafe\u0301", "café", ""].map do |name|
    { input: name, label: Event.event_type_label(name), color: helper.event_type_color_style(name) }
  end
  column_queries = [
    {}, { event_types: ["HTTP"] }, { event_types: ["HTTP", "payment_succeeded"] },
    { event_types: ["unknown"] }, { search: " sample 7 " }, { search: "%" },
    { search: "_" }, { search: "\\" }, { search: "amount" },
    { search: "unicode" }, { event_types: ["HTTP"], search: "sample 7" }
  ].flat_map do |filters|
    column = sink.columns.create!(name: "Reference column", config: { filters: filters })
    [nil, records[40].id, records[2].id].map do |anchor|
      { config: column.config, before_index: anchor && indexes[anchor],
        indexes: column.recent_events(before_id: anchor).map { |event| indexes.fetch(event.id) } }
    end
  end
  File.write(Rails.root.join("rust/tests/fixtures/feed-contracts.json"), JSON.pretty_generate({ inputs: inputs, queries: queries, labels: labels, column_queries: column_queries }) + "\n")
  puts "Exported #{queries.size} original Rails feed queries, #{column_queries.size} column queries and #{labels.size} label/color cases."
  raise ActiveRecord::Rollback
end
