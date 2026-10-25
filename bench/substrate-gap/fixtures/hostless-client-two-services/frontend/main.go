package main

import (
	"fmt"
	"log"
	"net/http"

	"github.com/gorilla/mux"
)

func homeHandler(w http.ResponseWriter, r *http.Request) {
	fmt.Fprint(w, "storefront")
}

func viewCartHandler(w http.ResponseWriter, r *http.Request) {
	fmt.Fprint(w, "cart")
}

func addToCartHandler(w http.ResponseWriter, r *http.Request) {
	http.Redirect(w, r, "/cart", http.StatusFound)
}

func main() {
	r := mux.NewRouter()
	r.HandleFunc("/", homeHandler).Methods("GET", "HEAD")
	r.HandleFunc("/cart", viewCartHandler).Methods("GET", "HEAD")
	r.HandleFunc("/cart", addToCartHandler).Methods("POST")
	log.Fatal(http.ListenAndServe(":8080", r))
}
