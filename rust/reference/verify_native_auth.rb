# bin/rails-reference runner --skip-executor rust/reference/verify_native_auth.rb
# Validates the native wire output with the ORIGINAL Rails authentication code.
require "json"
config = ActiveRecord::Base.connection_db_config.configuration_hash
abort "refusing non-reference database" unless config[:database] == "telesink_rust_reference" && config[:port].to_i == 55438
data = JSON.parse(File.read(Rails.root.join("tmp/rust-port/native-auth.json")))

ActiveRecord::Base.transaction do
  raise "fixture ID in use" if User.exists?(data.fetch("user_id")) || Session.exists?(data.fetch("session_id"))
  account = Account.create!
  user = account.users.create!(id: data.fetch("user_id"), email_address: "native-auth-#{SecureRandom.hex(8)}@example.test",
    password_digest: data.fetch("password_digest"), nickname: "Native", role: :owner)
  raise "native bcrypt rejected" unless user.authenticate(data.fetch("password"))
  raise "native reset token rejected" unless User.find_by_password_reset_token!(data.fetch("reset_token")) == user
  raise "native transfer rejected" unless User.find_by_transfer_id(data.fetch("transfer_token")) == user
  user.sessions.create!(id: data.fetch("session_id"))
  http = ActionDispatch::Integration::Session.new(Rails.application)
  http.cookies["session_id"] = Rack::Utils.unescape(data.fetch("session_cookie"))
  http.get("/registration/new")
  raise "native HTTP session rejected (#{http.response.status})" unless http.response.status == 302 && URI(http.response.location).path == "/"
  user.update!(password: "changed-after-native")
  raise "reset survived password change" if User.find_by_password_reset_token(data.fetch("reset_token"))
  puts "Rails accepted native bcrypt, reset and transfer links, and the native cookie through HTTP; password change invalidates the native reset link."
  raise ActiveRecord::Rollback
end
