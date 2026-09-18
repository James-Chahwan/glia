import requests


def load_users():
    return requests.get("http://api/users").json()
