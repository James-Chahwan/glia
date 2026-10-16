package test

import "github.com/gin-gonic/gin"

// NewTestRouter is a router the integration tests drive.
func NewTestRouter() *gin.Engine {
	r := gin.New()
	r.GET("/health-test", func(c *gin.Context) {})
	return r
}
