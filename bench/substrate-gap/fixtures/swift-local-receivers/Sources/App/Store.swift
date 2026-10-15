struct Store {
    func save(_ x: Int) {}
    func load() -> Int { return 1 }
    static func make() -> Store { return Store() }
}
