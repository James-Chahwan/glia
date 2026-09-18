package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func h(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := chi.NewRouter()
	r.Get("/feign/users/{id}", h)
	r.Get("/exchange/users/{id}", h)
	r.Post("/repos", h)
	http.ListenAndServe(":8080", r)
}
