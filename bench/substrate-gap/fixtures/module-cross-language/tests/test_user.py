from api.user import validate


def test_validate():
    assert validate(1) == 1
