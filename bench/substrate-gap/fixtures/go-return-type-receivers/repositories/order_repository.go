package repositories

// OrderRepository repeats every method name, so no call binds by a
// repo-unique method name.
type OrderRepository struct{}

func (r *OrderRepository) FindByID(id string) string { return id }
func (r *OrderRepository) Save(u string) error      { return nil }
func (r *OrderRepository) Count() int               { return 0 }
