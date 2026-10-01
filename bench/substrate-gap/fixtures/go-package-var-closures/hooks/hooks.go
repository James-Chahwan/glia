package hooks

import "example.com/hooks/store"

// newStore is swapped by tests.
var newStore = func() *store.Store { return store.Open() }

var (
	timeNow = func() int64 { return clock() }
	onEvict = func(key string) {
		store.Evict(key)
		audit(key)
	}
)

var handlers = map[string]func(){
	"flush": func() { flush() },
}

// defaultStore is opened once, at package init.
var defaultStore = store.Open()

var (
	limit, ttl = clamp(10), clamp(20)
	lo, hi     = bounds()
)

func clock() int64  { return 0 }
func audit(k string) {}
func flush()         {}
func clamp(n int) int { return n }
func bounds() (int, int) { return 0, 1 }

// Run calls the package vars like functions.
func Run() {
	s := newStore()
	_ = s
	_ = timeNow()
	onEvict("k")
}
