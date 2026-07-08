fn helper(x: i32) -> i32 {
    x + 1
}

pub fn compute(n: i32) -> i32 {
    helper(n) + helper(n)
}
