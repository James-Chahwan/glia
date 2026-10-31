protocol Store {
    func load() -> String
}

final class Mem: Store {
    func load() -> String { "" }
}

func makeStore() -> some Store {
    Mem()
}
