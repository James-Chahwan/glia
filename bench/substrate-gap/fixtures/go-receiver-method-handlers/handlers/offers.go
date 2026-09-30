package handlers

import "github.com/gin-gonic/gin"

type OffersHandler struct{}

func (h *OffersHandler) RegisterRoutes(protected *gin.RouterGroup) {
	protected.GET("/offers", h.List)
	protected.POST("/offers", h.Create)
}

func (h *OffersHandler) List(c *gin.Context)   {}
func (h *OffersHandler) Create(c *gin.Context) {}
