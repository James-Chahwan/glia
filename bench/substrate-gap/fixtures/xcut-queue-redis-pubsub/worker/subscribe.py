import json

import redis

r = redis.Redis(host="localhost", port=6379)


def run():
    p = r.pubsub()
    p.subscribe("notifications")
    for message in p.listen():
        if message["type"] == "message":
            handle(json.loads(message["data"]))


def handle(payload):
    print(payload)
