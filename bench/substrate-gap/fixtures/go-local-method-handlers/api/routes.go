package api

import "github.com/gin-gonic/gin"

func Register(r *gin.Engine, h *TokensHandler) {
	r.GET("/tokens", h.List)
}

func Register2(r *gin.Engine) {
	h := NewTokensHandler()
	r.POST("/tokens", h.Create)
	t := &TokensHandler{}
	r.GET("/tokens/:id", t.Show)
	var d TokensHandler
	r.DELETE("/tokens/:id", d.Del)
}
