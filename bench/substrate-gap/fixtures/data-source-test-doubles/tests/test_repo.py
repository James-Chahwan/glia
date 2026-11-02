import sqlalchemy as sa


def test_counts_rows(session):
    total = session.execute(sa.select(sa.func.count())).scalar_one()
    assert total == 0
