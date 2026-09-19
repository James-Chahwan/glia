import subprocess


def run():
    subprocess.run(["shipit", "deploy"], check=True)
