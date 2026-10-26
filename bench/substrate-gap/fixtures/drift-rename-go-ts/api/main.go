package main

import (
	"github.com/gin-gonic/gin"

	"example.com/inbox/api/handlers"
)

func main() {
	r := gin.Default()
	r.GET("/api/messages/:id", handlers.GetMessage)
	r.GET("/api/messages", handlers.ListMessages)
	_ = r.Run(":8080")
}
