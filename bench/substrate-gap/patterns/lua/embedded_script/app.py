import redis

r = redis.Redis()
rate_limit = r.register_script(open("scripts/rate_limit.lua").read())


def allow(user):
    return rate_limit(keys=[f"rl:{user}"], args=[60]) <= 10
