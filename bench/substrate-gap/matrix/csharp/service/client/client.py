import requests


def load_users():
    return requests.get("http://api/api/users").json()
