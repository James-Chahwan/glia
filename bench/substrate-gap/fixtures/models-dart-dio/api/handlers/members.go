package handlers

import (
	"net/http"

	"example.com/club/api/models"
	"github.com/gin-gonic/gin"
)

type Members struct {
	rows map[string]*models.Member
}

func (h *Members) Register(r *gin.Engine) {
	r.GET("/api/members/:id", h.Get)
	r.GET("/api/members", h.List)
	r.PUT("/api/members/:id", h.Rename)
}

func (h *Members) find(id string) (*models.Member, bool) {
	m, ok := h.rows[id]
	return m, ok
}

func (h *Members) all() []models.Member {
	out := make([]models.Member, 0, len(h.rows))
	for _, m := range h.rows {
		out = append(out, *m)
	}
	return out
}

func (h *Members) Get(c *gin.Context) {
	m, ok := h.find(c.Param("id"))
	if !ok {
		c.JSON(http.StatusNotFound, gin.H{"error": "not found"})
		return
	}
	c.JSON(http.StatusOK, m)
}

func (h *Members) List(c *gin.Context) {
	c.JSON(http.StatusOK, gin.H{"members": h.all()})
}

func (h *Members) Rename(c *gin.Context) {
	var req struct {
		Handle string `json:"handle" binding:"required"`
	}
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	c.Status(http.StatusNoContent)
}
