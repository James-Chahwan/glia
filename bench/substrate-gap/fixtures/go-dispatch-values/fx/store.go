package fx

// Store is a second interface named Store: `docstore.Store` must not bind here.
type Store interface {
	Put(key string) error
}

type MemStore struct{}

func (MemStore) Put(key string) error { return nil }
