package main

import (
	"log"
	"net/http"
)

func writeHealth(w http.ResponseWriter) {
	w.WriteHeader(http.StatusOK)
}

func listUsers(w http.ResponseWriter, r *http.Request) {
	w.WriteHeader(http.StatusOK)
}

func main() {
	http.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		log.Println("health")
		writeHealth(w)
	})
	http.HandleFunc("/users", listUsers)
	log.Fatal(http.ListenAndServe(":8080", nil))
}
