"""requests client calling a flask route in the server dir (HTTP_CALLS)."""
import requests


def fetch_user(uid):
    r = requests.get(f"http://api/users/{uid}")
    return r.json()


def make_user(body):
    requests.post("http://api/users", json=body)
