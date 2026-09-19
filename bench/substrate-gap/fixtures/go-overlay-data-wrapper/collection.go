package repositories

import (
	"go.mongodb.org/mongo-driver/mongo"
)

// Collection is a typed handle over one Mongo collection.
type Collection[T any] struct {
	inner *mongo.Collection
}

// NewCollection is the project-local constructor every repository goes through.
func NewCollection[T any](client *mongo.Client, database string, name string) *Collection[T] {
	return &Collection[T]{inner: client.Database(database).Collection(name)}
}

// NewNamedCollection forwards a caller-chosen name: no literal, no entity.
func NewNamedCollection[T any](client *mongo.Client, database string, name string) *Collection[T] {
	return NewCollection[T](client, database, name)
}
