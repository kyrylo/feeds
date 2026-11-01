# Run through bin/rails-reference. Temporary databases contain synthetic data only.
require "json"
require "open3"

original = ActiveRecord::Base.connection_db_config.configuration_hash
abort "refusing non-reference database" unless original[:database] == "telesink_rust_reference" && original[:port].to_i == 55438
database = "telesink_rust_schema_reference_#{Process.pid}"
ActiveRecord::Base.connection.create_database(database)
begin
  ActiveRecord::Base.establish_connection(original.merge(database: database))
  connection = ActiveRecord::Base.connection
  context = ActiveRecord::Base.connection_pool.migration_context
  [20260224101644, 20260628010000].each do |version|
    context.migrate(version)
    binary = File.join(ENV.fetch("TELESINK_TEST_PG_BIN", "/usr/local/opt/postgresql@16/bin"), "pg_dump")
    ddl, error, status = Open3.capture3(binary, "--schema-only", "--no-owner", "--no-privileges", "--no-comments", "--dbname=postgresql://telesink@127.0.0.1:55438/#{database}")
    raise "schema export failed: #{error}" unless status.success?
    versions = context.get_all_versions.sort
    ddl << "\nINSERT INTO public.schema_migrations(version) VALUES #{versions.map { |v| "('#{v}')" }.join(',')};\n"
    File.write(Rails.root.join("rust/tests/fixtures/schema-#{version}.sql"), ddl)
  end
  account = connection.select_value("INSERT INTO accounts (join_code,created_at,updated_at) VALUES ('schema-reference',now(),now()) RETURNING id")
  sink = connection.select_value("INSERT INTO sinks (account_id,name,token,created_at,updated_at) VALUES (#{account},'Reference','schema-reference-token',now(),now()) RETURNING id")
  inputs = [
    { event_type: "payment", text: " hello\nworld ", properties: { amount: 125.5, absent: nil, flag: false, nested: { list: [1, "two"] } } },
    { event_type: "quote", text: "Unicode 日本語 café", properties: { link: "<tag>&value", quoted: "\"\\", empty: "", array: [true, nil, " x "] } },
    { event_type: "event", text: "spaces\u00a0inside", properties: {} }
  ]
  inputs.each do |row|
    row[:id] = connection.select_value("INSERT INTO events (sink_id,event_type,text,properties,occurred_at,created_at,updated_at) VALUES (#{sink},#{connection.quote(row[:event_type])},#{connection.quote(row[:text])},#{connection.quote(row[:properties].to_json)}::jsonb,now(),now(),now()) RETURNING id")
  end
  context.migrate
  inputs.each do |row|
    row[:search_text] = connection.select_value("SELECT search_text FROM events WHERE id=#{row[:id]}")
  end
  File.write(Rails.root.join("rust/tests/fixtures/schema-search.json"), JSON.pretty_generate(inputs) + "\n")
  puts "Exported two original historical schemas and #{inputs.length} Rails search backfills."
ensure
  ActiveRecord::Base.connection_pool.disconnect!
  ActiveRecord::Base.establish_connection(original)
  ActiveRecord::Base.connection.drop_database(database)
end
