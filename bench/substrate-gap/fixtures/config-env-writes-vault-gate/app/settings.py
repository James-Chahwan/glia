import os
import hvac


def db_url():
    return os.environ["DATABASE_URL"]


def api_token():
    client = hvac.Client()
    client.secrets.kv.read_secret_version(path="api", mount_point="secret")
    return client.read("secret/data/billing")
