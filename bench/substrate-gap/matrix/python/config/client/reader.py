import os

import psycopg

DB_URL = os.environ["DB_URL"]


def connect():
    return psycopg.connect(DB_URL)
