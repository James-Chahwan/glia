package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func getUser(w http.ResponseWriter, r *http.Request)    {}
func createUser(w http.ResponseWriter, r *http.Request) {}
func listOrders(w http.ResponseWriter, r *http.Request) {}
func deleteUser(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := chi.NewRouter()
	r.Get("/api/users/{id}", getUser)
	r.Post("/api/users", createUser)
	r.Get("/api/orders", listOrders)
	r.Delete("/api/users/{id}", deleteUser)
	http.ListenAndServe(":8080", r)
}
