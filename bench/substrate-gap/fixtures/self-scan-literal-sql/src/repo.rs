// Repository layer: one real query, plus test strings and a comment that
// only quote SQL / Cypher text.
// SELECT * FROM phantoms

pub struct Db;

impl Db {
    pub fn query(&self, _sql: &str) {}
}

pub fn load(db: &Db) {
    db.query("SELECT id FROM accounts");
}

#[cfg(test)]
mod t {
    const SRC: &str = "db.query(\"SELECT * FROM ghosts\")";
    const GQL: &str = "MATCH (a:FUNCTION) RETURN a";

    #[test]
    fn samples_are_text() {
        assert!(SRC.contains("ghosts") && GQL.contains("FUNCTION"));
    }
}
