package main

import (
	"database/sql"
	"net/http"

	"github.com/go-chi/chi/v5"
)

func listUsers(db *sql.DB) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		rows, err := db.Query("SELECT id FROM users")
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		defer rows.Close()
		w.WriteHeader(http.StatusOK)
	}
}

func main() {
	r := chi.NewRouter()
	r.Get("/users", listUsers(nil))
	http.ListenAndServe(":8080", r)
}
