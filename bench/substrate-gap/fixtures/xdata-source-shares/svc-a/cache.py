import redis

client = redis.Redis(host="cache")


def get_session(sid):
    return client.get(sid)
