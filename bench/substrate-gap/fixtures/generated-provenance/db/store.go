package db

// NewStore is hand-written; it calls the generated query.
func NewStore() string {
	return GetUser(1)
}
