import subprocess


def run():
    subprocess.run(["mix", "shop.sync"], check=True)
