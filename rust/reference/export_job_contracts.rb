# Real Solid Queue rows, generated only in a disposable reference database.
require "json"
require "open3"

original = ActiveRecord::Base.connection_db_config.configuration_hash
abort "refusing non-reference database" unless original[:database] == "telesink_rust_reference" && original[:port].to_i == 55438
database = "telesink_rust_queue_reference_#{Process.pid}"
ActiveRecord::Base.connection.create_database(database)
begin
  queue_config = original.merge(database: database)
  ActiveRecord::Base.establish_connection(queue_config)
  load Rails.root.join("db/queue_schema.rb")
  binary = File.join(ENV.fetch("TELESINK_TEST_PG_BIN", "/usr/local/opt/postgresql@16/bin"), "pg_dump")
  ddl, error, status = Open3.capture3(binary, "--schema-only", "--no-owner", "--no-privileges", "--no-comments", "--dbname=postgresql://telesink@127.0.0.1:55438/#{database}")
  raise "queue schema export failed: #{error}" unless status.success?
  File.write(Rails.root.join("rust/tests/fixtures/queue-schema.sql"), ddl)
  ActiveRecord::Base.establish_connection(original)
  SolidQueue::Record.establish_connection(queue_config)
  [ProcessEventJob, ActionMailer::MailDeliveryJob, Turbo::Streams::BroadcastStreamJob, SolidQueue::RecurringJob].each { |klass| klass.queue_adapter = :solid_queue }
  ActiveRecord::Base.transaction do
    account = Account.create!
    user = account.users.create!(email_address: "queue-reference@example.test", password: "synthetic-password", role: :owner)
    sink = account.sinks.create!(name: "Queue reference")
    sink.sink_memberships.create!(user: user)
    ProcessEventJob.perform_later(sink, { text: "Accepted before cutover", event: "payment", properties: { amount: 125, absent: nil, nested: { list: [1, "two"] } }, sdk: { name: "reference", version: "1.0" } }.with_indifferent_access)
    ProcessEventJob.perform_later(sink, { text: "Retry with a key", idempotency_key: "reference-key", occurred_at: "2026-10-18T01:02:03.456Z" }.with_indifferent_access)
    ProcessEventJob.set(wait: 1.hour).perform_later(sink, { text: "Future event" }.with_indifferent_access)
    PasswordsMailer.reset(user).deliver_later
    Turbo::Streams::BroadcastStreamJob.perform_later("user_#{user.id}_sinks", content: "<turbo-stream action=\"refresh\"></turbo-stream>")
    SolidQueue::RecurringJob.perform_later("SolidQueue::Job.clear_finished_in_batches(sleep_between_batches: 0.3)")
    output = { sink_id: sink.id, user_id: user.id, jobs: SolidQueue::Job.order(:id).map(&:attributes) }
    File.write(Rails.root.join("rust/tests/fixtures/queue-jobs.json"), JSON.pretty_generate(output) + "\n")
    puts "Exported #{output[:jobs].length} original Solid Queue jobs and its actual schema."
    raise ActiveRecord::Rollback
  end
ensure
  SolidQueue::Record.connection_pool.disconnect! if defined?(SolidQueue::Record)
  ActiveRecord::Base.connection_pool.disconnect!
  ActiveRecord::Base.establish_connection(original)
  ActiveRecord::Base.connection.drop_database(database)
end
