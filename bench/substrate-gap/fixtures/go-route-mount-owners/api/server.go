package api

import "github.com/gin-gonic/gin"

type Server struct {
	Public *gin.RouterGroup
}

func (s *Server) Routes() {
	s.Public.GET("/items", s.items)
}

func (s *Server) items(c *gin.Context) {}
