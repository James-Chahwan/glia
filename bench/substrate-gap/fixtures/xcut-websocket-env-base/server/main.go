package main

import (
	"github.com/gin-gonic/gin"
	"github.com/gorilla/websocket"
)

var upgrader = websocket.Upgrader{}

// feed upgrades GET /rt/feed and GET /rt/prices to the feed socket.
func feed(c *gin.Context) {
	conn, err := upgrader.Upgrade(c.Writer, c.Request, nil)
	if err != nil {
		return
	}
	defer conn.Close()
}

func main() {
	r := gin.Default()
	rt := r.Group("/rt")
	rt.GET("/feed", feed)
	rt.GET("/prices", feed)
	// The decoy: a route whose path is the client's template TAIL alone.
	r.GET("/prices", feed)
	r.Run(":8080")
}
