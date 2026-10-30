def invoices(conn):
    return conn.execute("SELECT id FROM invoices").fetchall()


def customers(session):
    return session.run("MATCH (u:Customer) RETURN u")
