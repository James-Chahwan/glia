package handlers

import "github.com/gin-gonic/gin"

// The receiver types are declared in tokens.go / offers.go: a split receiver.
func (h *TokensHandler) RegisterRoutes(r *gin.Engine) {
	r.GET("/tokens", h.List)
	r.POST("/tokens", h.Create)
	r.GET("/tokens/health", h.Health)
}

func (h *OffersHandler) RegisterRoutes(r *gin.Engine) {
	r.GET("/offers", h.List)
	r.POST("/offers", h.Create)
}
