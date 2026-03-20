require "test_helper"

class ColumnTest < ActiveSupport::TestCase
  test "filters returns empty hash when config is nil" do
    columns(:all_primary_account).update!(config: nil)
    assert_equal({}, columns(:all_primary_account).filters)
  end

  test "filters returns the filters hash" do
    columns(:all_primary_account).update!(config: {
      "filters" => {
        "event_types" => [ "user.signup" ],
        "search" => "premium"
      }
    })

    filters = { "event_types" => [ "user.signup" ], "search" => "premium" }
    assert_equal filters, columns(:all_primary_account).filters
  end

  test "event_types returns array (handles missing key)" do
    columns(:all_primary_account).update!(config: { "filters" => {} })

    assert_equal [], columns(:all_primary_account).event_types
  end

  test "search_term returns stripped string or nil" do
    columns(:all_primary_account).update!(config: {
      "filters" => { "search" => "   premium   " }
    })
    assert_equal "premium", columns(:all_primary_account).search_term

    columns(:all_primary_account).update!(config: {
      "filters" => { "search" => "   " }
    })
    assert_nil columns(:all_primary_account).search_term
  end

  test "recent_events applies filters and respects limit" do
    columns(:all_primary_account).update!(config: {
      "filters" => { "search" => "premium" }
    })

    events = columns(:all_primary_account).recent_events(limit: 2)
    assert_equal 2, events.size
    assert events.all? { |e| e.text.downcase.include?("premium") }
  end

  test "recent_events defaults to limit 30" do
    columns(:all_primary_account).update!(config: { "filters" => {} })

    events = columns(:all_primary_account).recent_events
    assert_equal 6, events.size
  end
end
