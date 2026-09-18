import subprocess


def run():
    subprocess.run(["invctl", "export"], check=True)
