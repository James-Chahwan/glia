package app

import "github.com/google/wire"

type UserStore interface{ Get(id string) string }

type UserRepo struct{}

func (r *UserRepo) Get(id string) string { return id }

func NewUserRepo() *UserRepo { return &UserRepo{} }

type UserService struct{ store UserStore }

func NewUserService(s UserStore) *UserService { return &UserService{store: s} }

func InitializeUserService() *UserService {
	wire.Build(NewUserService, NewUserRepo, wire.Bind(new(UserStore), new(*UserRepo)))
	return nil
}
