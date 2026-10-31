package main

import (
	"log"
	"net/http"
)

func serveecho(w http.ResponseWriter, r *http.Request) {
	log.Println("echo")
}

func main() {
	http.HandleFunc("/echo", serveecho)
	log.Fatal(http.ListenAndServe(":8080", nil))
}
