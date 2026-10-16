package main

import (
	"net/http/httptest"
	"testing"

	"github.com/gin-gonic/gin"
)

func TestRoutes(t *testing.T) {
	r := gin.New()
	r.GET("/api/orders", func(c *gin.Context) {})
	Register(r)
	r.GET("/boom", func(c *gin.Context) { panic("boom") })
	r.POST("/api/auth/password/reset/:token", func(c *gin.Context) {})
	_ = httptest.NewRecorder()
}
