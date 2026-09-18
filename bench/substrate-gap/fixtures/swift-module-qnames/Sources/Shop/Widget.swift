class Widget {
    func run() -> Int {
        return helper()
    }

    func helper() -> Int {
        return 1
    }
}

extension Widget {
    func extra() -> Int {
        return 2
    }
}

private enum Constants {
    static let x = 1
}
