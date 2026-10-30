//! Vault.sol moves to contracts/core/ between the two exports.

struct Scratch;

impl Scratch {
    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(rel).unwrap()
    }

    fn edit(&self) {
        let orders = self.read("svc/orders.py");
        let _ = orders;
    }
}

fn vault(old: &str) -> String {
    format!("contracts::Vault{old}")
}
