package repositories

import "go.mongodb.org/mongo-driver/mongo"

type User struct{ ID string }

type UserRepository struct {
	collection *Collection[User]
}

// Built through the forwarding wrapper: its literal reaches .Collection(name)
// in two hops.
func NewUserRepository(client *mongo.Client, database string) *UserRepository {
	return &UserRepository{collection: NewNamedCollection[User](client, database, "users")}
}
