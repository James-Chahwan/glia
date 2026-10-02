package handlers

import "github.com/gin-gonic/gin"

type TokensHandler struct {
	BaseHandler
}

func (h *TokensHandler) List(c *gin.Context)   {}
func (h *TokensHandler) Create(c *gin.Context) {}
