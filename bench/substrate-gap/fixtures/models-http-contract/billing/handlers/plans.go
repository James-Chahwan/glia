package handlers

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

type planBody struct {
	ID         string `json:"id"`
	Name       string `json:"name"`
	PriceCents int64  `json:"priceCents"`
}

func Register(r *gin.Engine) {
	r.GET("/api/plans/:id", GetPlan)
}

// GetPlan godoc
// @Router /api/plans/{id} [get]
func GetPlan(c *gin.Context) {
	c.JSON(http.StatusOK, planBody{ID: c.Param("id"), Name: "basic", PriceCents: 900})
}
