import os
import pathlib
import tempfile


def main():
    with tempfile.TemporaryDirectory() as tmp:
        gitconfig = pathlib.Path(tmp) / "gitconfig"
        os.environ["GIT_CONFIG_GLOBAL"] = str(gitconfig)
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        del os.environ["GIT_DIR"]
        os.environ.pop("GIT_WORK_TREE", None)
        os.environ["PATH"] += ":/opt/bin"
        if os.environ["CI"] == "true":
            return 1
    return 0
