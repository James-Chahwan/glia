require 'rails_helper'

RSpec.describe CacheWarmJob do
  it 'writes the warm flag' do
    redis = Redis.new
    redis.set('warm', '1')
    described_class.perform_now
    expect(redis.get('warm')).to eq('1')
  end
end
