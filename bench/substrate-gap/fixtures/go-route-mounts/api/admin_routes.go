package api

import "github.com/gin-gonic/gin"

func (s *Server) adminRoutes() {
	s.admin.GET("/stats", s.stats)
}

func (s *Server) stats(c *gin.Context) {}
