package store

// Closer is a one-method interface: a name-only match pairs it with every
// type that has a Close().
type Closer interface {
	Close()
}

type Store interface {
	Get(key string) string
	Put(key, value string)
}
