from setuptools import setup

setup(
    name="setuptool",
    packages=["setuptool"],
    entry_points={
        "console_scripts": [
            "setuptool = setuptool.app:run",
        ],
    },
)
