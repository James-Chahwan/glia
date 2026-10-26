package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func listUsers(w http.ResponseWriter, r *http.Request)  {}
func createUser(w http.ResponseWriter, r *http.Request) {}
func deleteUser(w http.ResponseWriter, r *http.Request) {}
func listTeams(w http.ResponseWriter, r *http.Request)  {}

func main() {
	r := chi.NewRouter()
	r.Get("/api/users", listUsers)
	r.Post("/api/users", createUser)
	r.Delete("/api/users/{id}", deleteUser)
	r.Get("/api/v1/teams", listTeams)
	http.ListenAndServe(":8080", r)
}
