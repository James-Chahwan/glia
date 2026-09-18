package server

import "net/http"

func Register(mux *http.ServeMux) {
	mux.HandleFunc("/api/users", ListUsers)
}

func ListUsers(w http.ResponseWriter, r *http.Request) {}
