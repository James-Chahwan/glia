from pytest import fixture


def pytest_addoption(parser):
    parser.addoption("--myplug", action="store_true")


def pytest_markdown_docs_globals():
    return {}


@fixture
def myplug_value():
    return 1
