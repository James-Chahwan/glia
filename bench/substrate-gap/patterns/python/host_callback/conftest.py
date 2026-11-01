import pytest


def _marker_line():
    return "slow: marks slow tests"


def pytest_configure(config):
    config.addinivalue_line("markers", _marker_line())


def pytest_markdown_docs_globals():
    return {"answer": 42}


@pytest.fixture(autouse=True)
def fresh_settings():
    yield {}


def helper():
    return 1
