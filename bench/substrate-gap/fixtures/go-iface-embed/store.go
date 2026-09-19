package shop

type Reader interface {
	Get(id string) string
}

// Store embeds Reader: its method set is {Get, Put}.
type Store interface {
	Reader
	Put(id string, v string)
}

type Mem struct{}

func (m *Mem) Get(id string) string { return "" }

func (m *Mem) Put(id string, v string) {}
