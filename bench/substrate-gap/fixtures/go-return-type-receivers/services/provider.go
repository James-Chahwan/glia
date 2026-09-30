package services

import "example.com/shop/repositories"

// UserRepository shares its name with the STRUCT it returns (quokka's
// repository_provider.go shape).
func UserRepository() *repositories.UserRepository {
	return repositories.NewUserRepository()
}
