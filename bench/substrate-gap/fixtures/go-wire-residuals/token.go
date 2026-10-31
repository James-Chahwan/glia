package main

import (
	"github.com/gin-gonic/gin"
	"github.com/gin-gonic/gin/binding"
)

type TokenRequest struct {
	Grant string `json:"grant_type"`
}

func bindRequest(ctx *gin.Context, frm any) error {
	return ctx.ShouldBindWith(frm, binding.JSON)
}

func issueToken(c *gin.Context) {
	var t TokenRequest
	if err := bindRequest(c, &t); err != nil {
		return
	}
}
