import subprocess


def run():
    subprocess.run(["mytool", "sync"], check=True)
