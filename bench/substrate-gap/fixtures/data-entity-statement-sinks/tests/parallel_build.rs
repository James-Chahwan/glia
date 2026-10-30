use std::path::Path;

fn write(dir: &Path, rel: &str, body: &str) {
    std::fs::write(dir.join(rel), body).unwrap();
}

fn seed(dir: &Path) {
    write(dir, "db/migrations/V1__init.sql", "CREATE TABLE orders (id INT PRIMARY KEY);\n");
    std::fs::write(dir.join("db/seed.sql"), "INSERT INTO ledgers (id) VALUES (1);\n").unwrap();
}

#[test]
fn builds_in_parallel() {
    let dir = std::env::temp_dir();
    seed(&dir);
}
