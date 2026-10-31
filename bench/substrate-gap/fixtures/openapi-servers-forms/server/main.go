package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func main() {
	r := chi.NewRouter()
	r.Get("/v1/a", handleA)
	r.Get("/v2/b", handleB)
	r.Get("/api/c", handleC)
	http.ListenAndServe(":8080", r)
}

func handleA(w http.ResponseWriter, r *http.Request) {}
func handleB(w http.ResponseWriter, r *http.Request) {}
func handleC(w http.ResponseWriter, r *http.Request) {}
