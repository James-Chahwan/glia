package app

import "example.com/promo/store"

type Service struct {
	users *store.Users
}

func (s *Service) Shutdown() error { return s.users.Close() }

func (s *Service) Label() string { return s.users.Name() }

func Run(u *store.Users) error { return u.Close() }
