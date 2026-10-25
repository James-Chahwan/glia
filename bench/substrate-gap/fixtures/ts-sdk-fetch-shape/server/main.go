package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func getMyUser(w http.ResponseWriter, r *http.Request)      {}
func updateMyUser(w http.ResponseWriter, r *http.Request)   {}
func deleteActivity(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := chi.NewRouter()
	r.Get("/users/me", getMyUser)
	r.Put("/users/me", updateMyUser)
	r.Delete("/activities/{id}", deleteActivity)
	http.ListenAndServe(":8080", r)
}
