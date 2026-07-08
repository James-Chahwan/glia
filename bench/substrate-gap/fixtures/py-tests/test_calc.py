"""pytest test that exercises add() (TESTS)."""
from calc import add


def test_add():
    assert add(2, 3) == 5
