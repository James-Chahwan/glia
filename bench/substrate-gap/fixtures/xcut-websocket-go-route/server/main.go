package main

import (
	"net/http"
)

// The gorilla chat example's layout: the /ws route is registered with a
// func literal that hands off to serveWs in client.go.
func main() {
	hub := newHub()
	go hub.run()
	http.HandleFunc("/ws", func(w http.ResponseWriter, r *http.Request) {
		serveWs(hub, w, r)
	})
	registerEcho()
	http.ListenAndServe(":8080", nil)
}
