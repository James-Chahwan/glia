import logging
import os


def noise(cache):
    # a path-shaped string, but no method signal
    os.makedirs("/tmp/x", exist_ok=True)
    logging.info("/var/log/app")
    # a dict lookup on a path key is no HTTP call
    cache.get("/users/me")
    # a data: URL is never an endpoint, whatever the method signal
    fetch_url("data:text/plain;base64,aGVsbG8=", method="GET")


def fetch_url(target, method="GET"):
    return target, method
