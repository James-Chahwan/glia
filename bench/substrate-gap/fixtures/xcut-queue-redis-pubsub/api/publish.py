import json

import redis

r = redis.Redis(host="localhost", port=6379)


def notify(payload):
    r.publish("notifications", json.dumps(payload))
