import subprocess


def run():
    subprocess.run(["billing", "reconcile"], check=True)
