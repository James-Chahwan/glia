package shop

func (s *UserService) Get(id int) string {
	s.audit(id)
	return s.repo.Find(id)
}

func (s *UserService) audit(id int) {}
