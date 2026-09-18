package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func h(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := chi.NewRouter()
	r.Get("/users", h)
	r.Post("/orders", h)
	r.Get("/accounts", h)
	r.Delete("/invoices", h)
	http.ListenAndServe(":8080", r)
}
