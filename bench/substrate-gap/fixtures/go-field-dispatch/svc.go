package shop

type UserRepo struct{}

func (r *UserRepo) Find(id int) string { return "x" }

type UserService struct {
	repo *UserRepo
}

func (s *UserService) Get(id int) string {
	s.audit(id)
	return s.repo.Find(id)
}

func (s *UserService) audit(id int) {}
