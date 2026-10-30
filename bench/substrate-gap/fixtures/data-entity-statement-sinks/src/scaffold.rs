use std::path::Path;

pub fn scaffold(p: &Path) {
    std::fs::write(p.join("V1.sql"), "CREATE TABLE parcels (id INT);\n").unwrap();
}
