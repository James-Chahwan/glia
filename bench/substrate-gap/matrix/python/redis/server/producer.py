import redis

r = redis.Redis(host="localhost", port=6379)


def publish_order(payload):
    r.lpush("orders", payload)
