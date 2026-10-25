package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func main() {
	r := chi.NewRouter()
	r.Get("/api/users/list", listUsers)
	r.Get("/api/orders/recent", listOrders)
	http.ListenAndServe(":8080", r)
}

func listUsers(w http.ResponseWriter, req *http.Request)  {}
func listOrders(w http.ResponseWriter, req *http.Request) {}
