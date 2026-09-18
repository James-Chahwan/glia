package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

type UserService struct{}

func (s *UserService) List() []string { return nil }

func main() {
	svc := &UserService{}
	r := chi.NewRouter()
	r.Get("/users", func(w http.ResponseWriter, req *http.Request) { svc.List() })
	http.ListenAndServe(":8080", r)
}
