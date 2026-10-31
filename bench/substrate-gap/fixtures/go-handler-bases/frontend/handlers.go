package main

import "net/http"

type frontendServer struct{}

func (fe *frontendServer) homeHandler(w http.ResponseWriter, r *http.Request)      {}
func (fe *frontendServer) addToCartHandler(w http.ResponseWriter, r *http.Request) {}
