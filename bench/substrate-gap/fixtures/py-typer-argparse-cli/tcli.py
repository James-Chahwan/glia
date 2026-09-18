import argparse

import typer

app = typer.Typer()


@app.command()
def import_users(path: str):
    pass


@app.command("purge")
def purge_all():
    pass


def run_migrate(args):
    pass


def main():
    parser = argparse.ArgumentParser(prog="dbtool")
    sub = parser.add_subparsers()
    m = sub.add_parser("migrate")
    m.set_defaults(func=run_migrate)
