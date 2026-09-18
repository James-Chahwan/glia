import subprocess


def run():
    subprocess.run(["php", "bin/console", "app:sync-orders"], check=True)
