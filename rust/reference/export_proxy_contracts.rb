# Independent Rails client-address resolution, with only synthetic IP addresses.
require "json"
abort "reference only" unless ActiveRecord::Base.connection_db_config.database == "telesink_rust_reference"
cases = [
  ["127.0.0.1", nil, nil],
  ["172.18.0.2", "198.51.100.10", nil],
  ["10.0.0.5", "198.51.100.10, 192.168.1.2, 172.20.1.1", nil],
  ["127.0.0.1", "203.0.113.1, 198.51.100.10", nil],
  ["::1", "2001:db8::20, fd00::1, fe80::1", nil],
  ["10.0.0.5", "10.1.1.1, 192.168.1.2", nil],
  ["127.0.0.1", "garbage, 192.0.2.0/24, 198.51.100.10", nil],
  ["127.0.0.1", nil, "198.51.100.10"],
  ["127.0.0.1", "198.51.100.10, 10.0.0.1", "198.51.100.10"],
  ["127.0.0.1", "198.51.100.10", "203.0.113.20"]
].map do |peer, forwarded, client|
  request = ActionDispatch::Request.new({ "REMOTE_ADDR" => peer, "HTTP_X_FORWARDED_FOR" => forwarded, "HTTP_CLIENT_IP" => client }.compact)
  row = { peer:, forwarded:, client: }
  begin
    row[:expected] = ActionDispatch::RemoteIp::GetIp.new(request, true, ActionDispatch::RemoteIp::TRUSTED_PROXIES).to_s
  rescue ActionDispatch::RemoteIp::IpSpoofAttackError
    row[:rejected] = true
  end
  row
end
File.write(Rails.root.join("rust/tests/fixtures/proxy.json"), JSON.pretty_generate(cases) + "\n")
puts "Exported #{cases.length} original proxy cases."
