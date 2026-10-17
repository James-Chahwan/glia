package docstore

// Store persists uploaded documents.
type Store interface {
	Put(key string) error
}

// LocalStore writes to the local disk.
type LocalStore struct{}

func (LocalStore) Put(key string) error { return nil }

// Purge is called by nothing.
func (LocalStore) Purge() error { return nil }
