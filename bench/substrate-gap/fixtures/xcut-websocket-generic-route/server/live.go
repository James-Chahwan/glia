package main

import (
	"net/http"

	"github.com/gorilla/websocket"
)

var upgrader = websocket.Upgrader{}

// ServeLive upgrades the /live HTTP request to a WebSocket connection.
func ServeLive(w http.ResponseWriter, r *http.Request) {
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		return
	}
	defer conn.Close()
}

func main() {
	http.HandleFunc("/live", ServeLive)
	http.ListenAndServe(":8080", nil)
}
