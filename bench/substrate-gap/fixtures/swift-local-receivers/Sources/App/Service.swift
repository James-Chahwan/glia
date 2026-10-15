final class Service {
    func run(store: Store) {
        store.save(1)
    }

    func local() {
        let s = Store()
        s.save(2)
        let t: Store = Store.make()
        t.load()
        var u = Store.init()
        u.load()
        let o: Store? = nil
        o?.save(3)
    }
}

func freeRun(_ store: Store) {
    store.save(4)
}
