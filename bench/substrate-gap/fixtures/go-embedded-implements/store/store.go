package store

type Base struct{}

func (b *Base) Close() error { return nil }

type Closer interface {
	Close() error
}

type Repo interface {
	Save() error
	Close() error
}

// Users: own Save + Close promoted from *Base.
type Users struct {
	*Base
}

func (u *Users) Save() error { return nil }

// LoggingRepo embeds the Repo interface: Close is promoted from it.
type LoggingRepo struct {
	Repo
}

func (l LoggingRepo) Save() error { return l.Repo.Save() }

// Pair embeds two types that both declare Close at depth 1: Go promotes
// neither, so Pair has no Close.
type A struct{}

func (A) Close() error { return nil }

type B struct{}

func (B) Close() error { return nil }

type Pair struct {
	A
	B
}
