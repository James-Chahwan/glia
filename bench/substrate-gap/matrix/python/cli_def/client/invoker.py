import subprocess


def run_sync():
    subprocess.run(["mytool", "sync"], check=True)
