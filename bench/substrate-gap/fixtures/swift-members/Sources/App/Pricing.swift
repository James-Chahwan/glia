func tax(_ x: Int) -> Int { return x / 10 }

final class Repo {
    func load() -> Int { return 1 }
}

final class Cart {
    let repo: Repo
    var items: [Int] = []

    init(repo: Repo) {
        self.repo = repo
        self.reset()
    }

    deinit {
        self.reset()
    }

    subscript(i: Int) -> Int {
        return self.scale(items[i])
    }

    var total: Int {
        return items.reduce(0, +) + tax(items.count)
    }

    func reset() { items = [] }

    func scale(_ v: Int) -> Int { return v * 2 }

    func checkout() -> Int {
        let base = 1 + tax(3)
        return base + self.repo.load()
    }
}
