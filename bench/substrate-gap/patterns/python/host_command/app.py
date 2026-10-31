# A Flask app registers a CLI command with its host (`flask seed-db`).
import click
from flask import Flask

app = Flask(__name__)


def seed_rows(count):
    return [{"id": i} for i in range(count)]


@app.cli.command("seed-db")
@click.argument("count")
def seed_db(count):
    click.echo(len(seed_rows(int(count))))
