package api

import "github.com/gin-gonic/gin"

type Server struct {
	admin *gin.RouterGroup
}

func NewServer(r *gin.Engine) *Server {
	s := &Server{}
	s.admin = r.Group("/admin")
	s.adminRoutes()
	return s
}
