package main

import (
	"encoding/json"
	"net/http"

	"github.com/gin-gonic/gin"
	"github.com/gin-gonic/gin/binding"
)

type Order struct {
	ID string `json:"id"`
}

type Refund struct {
	Reason string `json:"reason"`
}

type Filter struct {
	Page int `form:"page"`
}

func get(w http.ResponseWriter, r *http.Request) {
	var o Order
	enc := json.NewEncoder(w)
	enc.Encode(o)
}

func create(c *gin.Context) {
	var o Order
	c.ShouldBindWith(&o, binding.JSON)
}

func refund(c *gin.Context) {
	var rf Refund
	c.ShouldBindBodyWith(&rf, binding.JSON)
}

func search(c *gin.Context) {
	var f Filter
	c.ShouldBindWith(&f, binding.Query)
}
