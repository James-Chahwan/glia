package main

import (
	"log"
	"net/http"
)

func serveautobahn(w http.ResponseWriter, r *http.Request) {
	log.Println("autobahn")
}

func main() {
	http.HandleFunc("/autobahn", serveautobahn)
	log.Fatal(http.ListenAndServe(":8080", nil))
}
