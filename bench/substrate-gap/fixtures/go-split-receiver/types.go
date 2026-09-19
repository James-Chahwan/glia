package shop

type UserRepo struct{}

func (r *UserRepo) Find(id int) string { return "x" }

type UserService struct {
	repo *UserRepo
}
