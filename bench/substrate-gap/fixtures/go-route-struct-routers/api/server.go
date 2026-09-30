package api

import "github.com/gin-gonic/gin"

type Server struct {
	router *gin.Engine
	v1     *gin.RouterGroup
	client *Client
}

type Client struct{}

func (c *Client) Get(path string) {}

func NewServer() *Server {
	s := &Server{router: gin.New()}
	s.v1 = s.router.Group("/v1")
	s.routes()
	return s
}

func (s *Server) routes() {
	s.router.GET("/health", s.health)
	s.v1.GET("/orders", s.listOrders)
	s.v1.POST("/orders", s.createOrder)
	s.client.Get("/upstream")
}

func (s *Server) health(c *gin.Context)      {}
func (s *Server) listOrders(c *gin.Context)  {}
func (s *Server) createOrder(c *gin.Context) {}
