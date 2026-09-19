package shop

type MemStore struct{ data map[string]string }

func (m *MemStore) Get(id string) (string, error) { return m.data[id], nil }

func (m *MemStore) Put(id string, v string) error {
	m.data[id] = v
	return nil
}

// ReadOnly has Get but not Put, so it does not satisfy Store.
type ReadOnly struct{}

func (r ReadOnly) Get(id string) (string, error) { return "", nil }
