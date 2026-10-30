def test_seed(conn):
    conn.execute("INSERT INTO fixtures_seed (id) VALUES (1)")
