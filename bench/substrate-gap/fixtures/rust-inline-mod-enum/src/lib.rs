pub enum MatchTier { Exact, BaseFold, Suffix(u8), Named { n: u32 } }

impl MatchTier {
    pub fn rank(&self) -> u8 { self.weight() }
    fn weight(&self) -> u8 { 1 }
}

pub fn helper() -> u8 { 0 }

pub mod endpoint {
    pub fn url_to_path(s: &str) -> String { let _ = helper(); s.to_string() }
    fn helper() -> u8 { 1 }
    pub mod inner {
        pub fn deep() {}
    }
}

pub fn tier() -> MatchTier { MatchTier::BaseFold }
pub fn mk() -> MatchTier { MatchTier::Suffix(3) }
pub fn named() -> MatchTier { MatchTier::Named { n: 1 } }
pub fn use_ep() -> String { endpoint::url_to_path("x") }
pub fn use_inner() { endpoint::inner::deep() }
pub fn classify(t: MatchTier) -> u8 {
    match t { MatchTier::Exact => 0, MatchTier::Suffix(_) => 2, _ => 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tier_is_base_fold() { let _ = tier(); }
}
