import subprocess


def run():
    subprocess.run(["shopctl", "sync-users", "--force"], check=True)
