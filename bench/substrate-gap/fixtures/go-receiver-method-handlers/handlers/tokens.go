package handlers

import "github.com/gin-gonic/gin"

type TokensHandler struct{}

func (h *TokensHandler) RegisterRoutes(public *gin.RouterGroup) {
	public.GET("/tokens", h.List)
	public.POST("/tokens", h.Create)
}

func (h *TokensHandler) List(c *gin.Context)   {}
func (h *TokensHandler) Create(c *gin.Context) {}
