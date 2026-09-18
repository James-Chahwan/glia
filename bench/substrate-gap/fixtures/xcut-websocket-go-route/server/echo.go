package main

import (
	"net/http"

	"nhooyr.io/websocket"
)

// echo accepts a WebSocket on /echo and closes it.
func echo(w http.ResponseWriter, r *http.Request) {
	c, err := websocket.Accept(w, r, nil)
	if err != nil {
		return
	}
	defer c.Close(websocket.StatusNormalClosure, "")
}

func registerEcho() {
	mux := http.NewServeMux()
	mux.HandleFunc("/echo", echo)
	_ = mux
}
