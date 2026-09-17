import redis

r = redis.Redis(host="localhost", port=6379)


def run():
    while True:
        _, payload = r.blpop("orders")
        handle(payload)


def handle(payload):
    print(payload)
