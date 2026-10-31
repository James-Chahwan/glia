func run(s: any Store) {
    _ = s.load()
}

func chain() {
    _ = makeStore().load()
}

func held() {
    let made = makeStore()
    _ = made.load()
}
