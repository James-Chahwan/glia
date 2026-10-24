"""Invoice store."""


def list_invoices(cur):
    """cur.execute('SELECT * FROM shadows')"""
    cur.execute("SELECT * FROM invoices")
    return cur.fetchall()
