package main

import (
	"net/http"
)

func updateMe(w http.ResponseWriter, r *http.Request) {
	w.WriteHeader(http.StatusNoContent)
}

func serverVersion(w http.ResponseWriter, r *http.Request) {
	w.Write([]byte("1.0.0"))
}

func main() {
	mux := http.NewServeMux()
	mux.HandleFunc("PUT /api/users/me", updateMe)
	mux.HandleFunc("GET /api/server/version", serverVersion)
	http.ListenAndServe(":8080", mux)
}
