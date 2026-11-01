# Export the unchanged Rack::Cors behavior; no database rows or jobs are created.
require "json"
abort "reference only" unless ActiveRecord::Base.connection_db_config.database == "telesink_rust_reference"
cases = [
  ["OPTIONS", "/api/v1/sinks/synthetic-no-such-token/events", "POST", "content-type,idempotency-key"],
  ["OPTIONS", "/api/v1/sinks/synthetic-no-such-token/events", "DELETE", "content-type"],
  ["OPTIONS", "/api/v1/sinks/synthetic-no-such-token/events", "POST", "x-not-allowed"],
  ["POST", "/api/v1/sinks/synthetic-no-such-token/events", nil, nil],
  ["GET", "/session/new", nil, nil]
].map do |method, path, requested_method, requested_headers|
  client = ActionDispatch::Integration::Session.new(Rails.application)
  headers = { "HTTP_HOST" => "localhost", "HTTP_ORIGIN" => "https://sender.example.test" }
  headers["HTTP_ACCESS_CONTROL_REQUEST_METHOD"] = requested_method if requested_method
  headers["HTTP_ACCESS_CONTROL_REQUEST_HEADERS"] = requested_headers if requested_headers
  client.public_send(method.downcase, path, headers: headers)
  { method:, path:, requested_method:, requested_headers:, status: client.response.status,
    headers: client.response.headers.to_h.select { |name, _| name.downcase.start_with?("access-control-") } }
end
File.write(Rails.root.join("rust/tests/fixtures/cors.json"), JSON.pretty_generate(cases) + "\n")
puts "Exported #{cases.length} original CORS cases."
