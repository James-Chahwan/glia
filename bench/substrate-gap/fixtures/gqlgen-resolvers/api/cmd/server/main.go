package main

import (
	"net/http"

	"example.com/shop/api/graph"
)

func main() {
	_ = &graph.Resolver{Stock: map[string]int{}}
	_ = http.ListenAndServe(":8080", nil)
}
