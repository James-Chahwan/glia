// Swift intra-file CALLS: compute() calls helper().
func helper(_ x: Int) -> Int {
    return x * 2
}

func compute(_ n: Int) -> Int {
    return helper(n) + 1
}
