package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func createUser(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := chi.NewRouter()
	r.Post("/api/users", createUser)
	http.ListenAndServe(":8080", r)
}
