package server

import "github.com/gin-gonic/gin"

var health = func(c *gin.Context) { writeHealth(c) }

func writeHealth(c *gin.Context) {}

func Routes(r *gin.Engine) {
	r.GET("/health", health)
}
