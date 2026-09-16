import click


@click.group()
def cli():
    """mytool — the fixture's console entry point."""


@click.command("sync")
@click.option("--force", is_flag=True)
def sync(force):
    """Sync records."""
    click.echo("syncing")


cli.add_command(sync)
