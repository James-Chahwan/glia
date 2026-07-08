package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

// Server side of the Dart-HTTP fixture: routes that the client's dio calls
// target. These ROUTE nodes exist today; the Dart ENDPOINT nodes do not, so
// the HTTP_CALLS cross-edge cannot form (that is the gap under test).
func getUser(w http.ResponseWriter, r *http.Request)    {}
func createUser(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := chi.NewRouter()
	r.Get("/users/{id}", getUser)
	r.Post("/users", createUser)
	http.ListenAndServe(":8080", r)
}
