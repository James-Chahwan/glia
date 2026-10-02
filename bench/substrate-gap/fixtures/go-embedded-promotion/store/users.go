package store

// Users gets Close, exec and the field db from the embedded *Base; its own
// Name shadows Base's.
type Users struct {
	*Base
	name string
}

func (u *Users) Name() string { return u.name }

func (u *Users) Save() error { return u.exec("insert") }

func (u *Users) Flush() error { return u.db.Run("flush") }
