require "test_helper"

class Settings::InvitationsControllerTest < ActionDispatch::IntegrationTest
  test "owner can view the invitations page" do
    owner = users(:test_owner)
    owner.update!(role: :owner)
    sign_in_as(owner)

    get settings_invitations_path

    assert_response :success
    assert_match join_url(owner.account.join_code), response.body
  end

  test "admin can view the invitations page" do
    admin = users(:test_owner)
    admin.update!(role: :admin)
    sign_in_as(admin)

    get settings_invitations_path

    assert_response :success
    assert_match join_url(admin.account.join_code), response.body
  end

  test "regular member cannot view the invitations page" do
    member = users(:test_owner)
    member.update!(role: :member)
    sign_in_as(member)

    get settings_invitations_path

    assert_response :forbidden
  end
end
