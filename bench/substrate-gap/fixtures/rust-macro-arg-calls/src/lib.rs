macro_rules! run {
    ($call:expr) => {{
        let _ = $call;
    }};
}

pub struct Wrap(pub u32);

pub fn helper(x: u32) -> u32 { x + 1 }
pub fn fmt_id(x: u32) -> String { format!("id-{}", x) }
pub fn total(v: Vec<u32>) -> u32 { v.len() as u32 }

pub fn entry() -> u32 {
    run!(helper(2));
    println!("{}", fmt_id(helper(1)));
    assert_eq!(helper(0), 1);
    total(vec![helper(3)])
}

pub fn quiet() { println!("fmt_id(1) is not a call"); }

pub fn is_wrap(w: Option<Wrap>) -> bool { matches!(w, Some(Wrap(_))) }

pub struct Svc;
impl Svc {
    pub fn go(&self) -> String { format!("{}", self.name()) }
    fn name(&self) -> String { String::new() }
}
