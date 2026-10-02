package handlers

import "github.com/gin-gonic/gin"

type OffersHandler struct {
	BaseHandler
}

func (h *OffersHandler) List(c *gin.Context)   {}
func (h *OffersHandler) Create(c *gin.Context) {}
