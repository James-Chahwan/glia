require "redis"

redis = Redis.new
redis.subscribe("orders") do |on|
  on.message { |_channel, msg| puts msg }
end
