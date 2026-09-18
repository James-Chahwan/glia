import subprocess


def run():
    subprocess.run(["deployer", "rollout", "prod"], check=True)
