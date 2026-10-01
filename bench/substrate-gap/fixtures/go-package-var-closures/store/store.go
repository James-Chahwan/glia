package store

type Store struct{}

func Open() *Store { return &Store{} }

func Evict(key string) {}
