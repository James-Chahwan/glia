package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func getView(w http.ResponseWriter, r *http.Request)     {}
func createView(w http.ResponseWriter, r *http.Request)  {}
func listCached(w http.ResponseWriter, r *http.Request)  {}
func listOrphans(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := chi.NewRouter()
	r.Get("/api/views/{id}", getView)
	r.Post("/api/views", createView)
	r.Get("/api/cached", listCached)
	r.Get("/api/orphans", listOrphans)
	http.ListenAndServe(":8080", r)
}
