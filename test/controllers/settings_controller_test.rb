require "test_helper"

class SettingsControllerTest < ActionDispatch::IntegrationTest
  setup do
    sign_in_as users(:test_owner)
  end

  test "show redirects to edit nickname page" do
    get settings_path
    assert_redirected_to edit_settings_nickname_path
  end
end
