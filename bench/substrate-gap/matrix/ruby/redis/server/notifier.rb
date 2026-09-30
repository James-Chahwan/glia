require "redis"

redis = Redis.new
redis.publish("orders", "order-1")
