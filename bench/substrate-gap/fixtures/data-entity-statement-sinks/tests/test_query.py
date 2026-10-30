import glia_py


def main(c, tmp):
    g = glia_py.generate(str(tmp))
    g.query_gql("MATCH (a:FUNCTION)-[:CALLS]->(b) WHERE a.qname STARTS WITH 'web::' " "RETURN a.qname, b.qname")
    c.raises("x", ValueError, lambda: g.query_gql("MATCH (a:ROUTEE) RETURN a"), "ROUTEE")
