package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func login(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := chi.NewRouter()
	r.Post("/login", login)
	http.ListenAndServe(":8080", r)
}
