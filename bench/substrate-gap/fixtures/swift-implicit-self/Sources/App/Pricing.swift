func helper() -> Int { return 0 }

final class Cart {
    func helper() -> Int { return 2 }

    func checkout() -> String {
        let n = helper()
        return Formatter.money(n)
    }
}
