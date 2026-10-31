package main

import (
	"log"
	"net/http"
)

func servebufferpool(w http.ResponseWriter, r *http.Request) {
	log.Println("bufferpool")
}

func main() {
	http.HandleFunc("/bufferpool", servebufferpool)
	log.Fatal(http.ListenAndServe(":8080", nil))
}
