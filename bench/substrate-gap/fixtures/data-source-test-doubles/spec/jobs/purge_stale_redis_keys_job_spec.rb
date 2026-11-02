require 'rails_helper'

RSpec.describe PurgeStaleRedisKeysJob do
  it 'enqueues a flush for every stale account' do
    expect(FlushRedisKeysJob).to receive(:perform_later).with(1)
    described_class.perform_now
  end
end
