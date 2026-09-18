pub enum Tier {
    Gold,
    Silver,
}

impl Tier {
    pub fn rank(&self) -> u8 {
        self.weight() + 1
    }

    fn weight(&self) -> u8 {
        1
    }
}

pub struct Ledger;

impl Ledger {
    pub fn total(&self) -> u8 {
        self.weight()
    }

    fn weight(&self) -> u8 {
        2
    }
}
