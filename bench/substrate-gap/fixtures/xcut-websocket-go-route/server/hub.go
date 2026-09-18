package main

import (
	"github.com/gorilla/websocket"
)

// Hub fans messages out to every connected socket.
type Hub struct {
	conns []*websocket.Conn
}

func newHub() *Hub {
	return &Hub{}
}

func (h *Hub) run() {}

func (h *Hub) register(c *websocket.Conn) {
	h.conns = append(h.conns, c)
}
