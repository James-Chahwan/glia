package shop

// Store is satisfied implicitly: no type names it.
type Store interface {
	Get(id string) (string, error)
	Put(id string, v string) error
}

func Use(s Store) {
	s.Get("x")
}
