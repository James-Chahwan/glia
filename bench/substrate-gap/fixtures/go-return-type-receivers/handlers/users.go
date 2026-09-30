package handlers

import (
	"example.com/shop/repositories"
	"example.com/shop/services"
)

var defaultRepo = repositories.NewUserRepository()

func GetUser(id string) string {
	return services.UserRepository().FindByID(id)
}

func SaveUser(u string) error {
	repo := services.UserRepository()
	return repo.Save(u)
}

func CountUsers(repo *repositories.UserRepository) int {
	return repo.Count()
}

func DefaultUser(id string) string {
	return defaultRepo.FindByID(id)
}
