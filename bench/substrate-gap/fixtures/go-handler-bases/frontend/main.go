package main

import (
	"net/http"
	"os"

	"github.com/gorilla/mux"
)

var baseUrl = ""

func main() {
	baseUrl = os.Getenv("BASE_URL")
	svc := new(frontendServer)
	r := mux.NewRouter()
	r.HandleFunc(baseUrl+"/", svc.homeHandler).Methods(http.MethodGet, http.MethodHead)
	r.HandleFunc(baseUrl+"/cart", svc.addToCartHandler).Methods(http.MethodPost)
	http.ListenAndServe(":8080", r)
}
