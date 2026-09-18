import subprocess


def deploy():
    subprocess.run(["mytool", "migrate", "--yes"], check=True)
