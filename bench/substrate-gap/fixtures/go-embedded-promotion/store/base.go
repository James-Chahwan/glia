package store

// Base is embedded by every repository.
type Base struct {
	db *DB
}

func (b *Base) Close() error { return nil }

func (b *Base) Name() string { return "base" }

func (b *Base) exec(q string) error { return b.db.Run(q) }

type DB struct{}

func (d *DB) Run(q string) error { return nil }
