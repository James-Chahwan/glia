package api

import (
	"net/http"

	"github.com/gorilla/websocket"
)

var upgrader = websocket.Upgrader{}

// ServeWs upgrades the /ws HTTP request to a WebSocket connection.
func ServeWs(w http.ResponseWriter, r *http.Request) {
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		return
	}
	defer conn.Close()
}

func main() {
	http.HandleFunc("/ws", ServeWs)
	http.ListenAndServe(":8080", nil)
}
