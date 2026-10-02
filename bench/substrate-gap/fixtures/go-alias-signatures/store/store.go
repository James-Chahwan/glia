package store

type Record struct {
	ID string
}

type Key = string

type RecordList = []*Record

type Repo interface {
	Get(id string) (*Record, error)
	List() RecordList
}

// memRepo writes Key where Repo writes string, and []*Record where Repo
// writes RecordList: both sides resolve to one signature.
type memRepo struct{}

func (m *memRepo) Get(id Key) (*Record, error) { return nil, nil }

func (m *memRepo) List() []*Record { return nil }

// badRepo's Get takes an int: no alias makes it a string.
type badRepo struct{}

func (b *badRepo) Get(id int) (*Record, error) { return nil, nil }

func (b *badRepo) List() RecordList { return nil }
