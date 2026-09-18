class Widget {
    func run() -> Int {
        return self.helper()
    }

    func later() {
        schedule { [weak self] in
            self?.refresh()
        }
    }

    func refresh() {}

    func build() -> Widget {
        return Self.make()
    }

    static func make() -> Widget {
        return Widget()
    }

    func helper() -> Int {
        return 1
    }
}

struct Part {
    func weight() -> Int {
        return self.base()
    }

    func base() -> Int {
        return 2
    }
}

enum Mode {
    case on, off

    func label() -> String {
        return self.describe()
    }

    func describe() -> String {
        return "mode"
    }
}

func helper() -> Int {
    return 0
}

func schedule(_ work: @escaping () -> Void) {
    work()
}
