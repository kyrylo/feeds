require "test_helper"

class SinkMembershipTest < ActiveSupport::TestCase
  test "last_viewed_at_for returns nil when the column has never been viewed" do
    column = Column.new(id: 999)

    assert_nil sink_memberships(:test_owner_primary_account).last_viewed_at_for(column)
  end

  test "last_viewed_at_for returns a Time object when the column has been viewed" do
    column = Column.new(id: 42)

    sink_memberships(:test_owner_primary_account).column_last_viewed_at = { "42" => "2026-04-20T14:30:00Z" }
    sink_memberships(:test_owner_primary_account).save!

    result = sink_memberships(:test_owner_primary_account).last_viewed_at_for(column)
    assert_kind_of ActiveSupport::TimeWithZone, result
    assert_equal Time.zone.parse("2026-04-20T14:30:00Z"), result
  end

  test "last_viewed_at_for gracefully handles invalid timestamp strings" do
    column = Column.new(id: 1)

    sink_memberships(:test_owner_primary_account).column_last_viewed_at = { "1" => "not-a-time" }
    sink_memberships(:test_owner_primary_account).save!

    assert_nil sink_memberships(:test_owner_primary_account).last_viewed_at_for(column)
  end

  test "mark_all_columns_viewed updates timestamps for every column in the sink" do
    sink = sinks(:primary_account)

    # ensure the sink has columns
    sink.columns.destroy_all
    col1 = sink.columns.create!(name: "Column 1", position: 1)
    col2 = sink.columns.create!(name: "Column 2", position: 2)

    travel_to Time.zone.parse("2026-04-20 15:00:00 UTC") do
      sink_memberships(:test_owner_primary_account).mark_all_columns_viewed
    end

    sink_memberships(:test_owner_primary_account).reload

    assert_equal "2026-04-20T15:00:00Z", sink_memberships(:test_owner_primary_account).column_last_viewed_at[col1.id.to_s]
    assert_equal "2026-04-20T15:00:00Z", sink_memberships(:test_owner_primary_account).column_last_viewed_at[col2.id.to_s]
  end

  test "mark_all_columns_viewed saves the record" do
    assert_changes -> { sink_memberships(:test_owner_primary_account).updated_at } do
      sink_memberships(:test_owner_primary_account).mark_all_columns_viewed
    end
  end

  test "nesting_prefix returns empty string when sink has no folder" do
    sink = Sink.create!(name: "Ungrouped Sink", account: accounts(:primary_account))
    membership = SinkMembership.create!(user: users(:test_owner), sink: sink)

    assert_equal "", membership.nesting_prefix
  end

  test "nesting_prefix returns └─ for the only sink inside a folder" do
    folder = Folder.create!(name: "My Folder", account: accounts(:primary_account))
    sink = Sink.create!(name: "Solo Sink", account: accounts(:primary_account), folder: folder)
    membership = SinkMembership.create!(user: users(:test_owner), sink: sink)

    assert_equal "└─", membership.nesting_prefix
  end

  test "nesting_prefix returns ├─ for all but the last sink (alphabetical order) inside a folder" do
    folder = Folder.create!(name: "Alpha Folder", account: accounts(:primary_account))

    sink_z = Sink.create!(name: "Z Sink", account: accounts(:primary_account), folder: folder)
    sink_a = Sink.create!(name: "A Sink", account: accounts(:primary_account), folder: folder)
    sink_m = Sink.create!(name: "M Sink", account: accounts(:primary_account), folder: folder)

    m_z = SinkMembership.create!(user: users(:test_owner), sink: sink_z)
    m_a = SinkMembership.create!(user: users(:test_owner), sink: sink_a)
    m_m = SinkMembership.create!(user: users(:test_owner), sink: sink_m)

    # after ordering by sink name ASC the order should be: A Sink, M Sink, Z Sink
    assert_equal "├─", m_a.nesting_prefix
    assert_equal "├─", m_m.nesting_prefix
    assert_equal "└─", m_z.nesting_prefix
  end

  test "nesting_prefix respects folder ordering (folders.name ASC NULLS LAST)" do
    folder_b = Folder.create!(name: "B Folder", account: accounts(:primary_account))
    folder_a = Folder.create!(name: "A Folder", account: accounts(:primary_account))

    sink_in_b = Sink.create!(name: "Sink in B", account: accounts(:primary_account), folder: folder_b)
    sink_in_a = Sink.create!(name: "Sink in A", account: accounts(:primary_account), folder: folder_a)

    membership_b = SinkMembership.create!(user: users(:test_owner), sink: sink_in_b)
    membership_a = SinkMembership.create!(user: users(:test_owner), sink: sink_in_a)

    assert_equal "└─", membership_a.nesting_prefix
    assert_equal "└─", membership_b.nesting_prefix
  end

  test "nesting_prefix works correctly when user belongs to multiple folders" do
    folder1 = Folder.create!(name: "Folder One", account: accounts(:primary_account))
    folder2 = Folder.create!(name: "Folder Two", account: accounts(:primary_account))

    s1 = Sink.create!(name: "First",  account: accounts(:primary_account), folder: folder1)
    s2 = Sink.create!(name: "Second", account: accounts(:primary_account), folder: folder1)
    s3 = Sink.create!(name: "Third",  account: accounts(:primary_account), folder: folder2)

    m1 = SinkMembership.create!(user: users(:test_owner), sink: s1)
    m2 = SinkMembership.create!(user: users(:test_owner), sink: s2)
    m3 = SinkMembership.create!(user: users(:test_owner), sink: s3)

    assert_equal "├─", m1.nesting_prefix
    assert_equal "└─", m2.nesting_prefix
    assert_equal "└─", m3.nesting_prefix
  end

  test "nesting_prefix returns empty string for ungrouped sinks even when other sinks are in folders" do
    folder = Folder.create!(name: "Some Folder", account: accounts(:primary_account))
    Sink.create!(name: "Folder Sink", account: accounts(:primary_account), folder: folder)

    ungrouped_sink = Sink.create!(name: "Ungrouped", account: accounts(:primary_account))
    ungrouped_membership = SinkMembership.create!(user: users(:test_owner), sink: ungrouped_sink)

    assert_equal "", ungrouped_membership.nesting_prefix
  end
end
