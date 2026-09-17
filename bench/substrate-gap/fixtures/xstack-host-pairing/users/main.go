package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func listUsers(w http.ResponseWriter, r *http.Request) {}

func getUser(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := chi.NewRouter()
	r.Get("/users", listUsers)
	r.Get("/users/{id}", getUser)
	http.ListenAndServe(":8080", r)
}
