package main

import (
	"github.com/gin-gonic/gin"
	"github.com/gorilla/websocket"
)

var upgrader = websocket.Upgrader{}

// Hub fans order updates out to every live socket.
type Hub struct{}

// Serve upgrades GET /live/orders to the order feed socket.
func (h *Hub) Serve(c *gin.Context) {
	conn, err := upgrader.Upgrade(c.Writer, c.Request, nil)
	if err != nil {
		return
	}
	defer conn.Close()
}

// chatWS upgrades GET /api/chat/ws to the chat socket.
func chatWS(c *gin.Context) {
	conn, err := upgrader.Upgrade(c.Writer, c.Request, nil)
	if err != nil {
		return
	}
	defer conn.Close()
}

// adminPage is a plain HTTP page: it never upgrades.
func adminPage(c *gin.Context) {}

func main() {
	h := &Hub{}
	r := gin.Default()
	r.GET("/live/orders", h.Serve)
	api := r.Group("/api")
	api.GET("/chat/ws", chatWS)
	r.GET("/admin/live", adminPage)
	r.Run(":8080")
}
