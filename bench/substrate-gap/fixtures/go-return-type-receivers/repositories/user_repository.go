package repositories

type UserRepository struct{}

func NewUserRepository() *UserRepository { return &UserRepository{} }

func (r *UserRepository) FindByID(id string) string { return id }
func (r *UserRepository) Save(u string) error      { return nil }
func (r *UserRepository) Count() int               { return 0 }
