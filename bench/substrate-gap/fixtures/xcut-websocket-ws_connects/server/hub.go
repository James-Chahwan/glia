package server

import (
	"net/http"

	"github.com/gorilla/websocket"
)

var upgrader = websocket.Upgrader{
	ReadBufferSize:  1024,
	WriteBufferSize: 1024,
}

// ServeWs upgrades the /ws HTTP request to a WebSocket connection.
func ServeWs(w http.ResponseWriter, r *http.Request) {
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		return
	}
	defer conn.Close()
	for {
		_, msg, err := conn.ReadMessage()
		if err != nil {
			break
		}
		conn.WriteMessage(websocket.TextMessage, msg)
	}
}

func main() {
	http.HandleFunc("/ws", ServeWs)
	http.ListenAndServe(":8080", nil)
}
