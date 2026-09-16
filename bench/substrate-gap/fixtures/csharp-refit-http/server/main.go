package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func getUser(w http.ResponseWriter, r *http.Request)    {}
func createUser(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := chi.NewRouter()
	r.Get("/api/users/{id}", getUser)
	r.Post("/api/users", createUser)
	http.ListenAndServe(":8080", r)
}
