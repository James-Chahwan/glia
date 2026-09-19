# neo4j Python driver: real Cypher in string literals.
from neo4j import GraphDatabase


def find_person(tx, name):
    return tx.run("MATCH (p:Person {name: $name}) RETURN p", name=name).single()


def upsert_company(tx, cid):
    tx.run(
        "MERGE (c:Company {id: $id}) "
        "RETURN c",
        id=cid,
    )
