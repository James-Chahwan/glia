import boto3
import hvac


def db_password():
    sm = boto3.client("secretsmanager")
    return sm.get_secret_value(SecretId="prod/db-creds")["SecretString"]


def api_token():
    c = hvac.Client()
    return c.read("secret/data/api")["data"]
