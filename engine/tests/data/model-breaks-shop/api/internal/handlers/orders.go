package handlers

import (
	"net/http"
	"time"

	"github.com/gin-gonic/gin"

	"github.com/example/shop/api/internal/models"
	"github.com/example/shop/api/internal/store"
)

type Orders struct {
	store *store.Memory
}

func NewOrders(st *store.Memory) *Orders {
	return &Orders{store: st}
}

func (h *Orders) Register(g *gin.RouterGroup) {
	g.GET("/orders/:id", h.Get)
	g.POST("/orders", h.Place)
	g.PATCH("/orders/:id/status", h.ChangeStatus)
}

func (h *Orders) Get(c *gin.Context) {
	o, ok := h.store.Order(c.Param("id"))
	if !ok {
		c.JSON(http.StatusNotFound, gin.H{"error": "not found"})
		return
	}
	c.JSON(http.StatusOK, o)
}

func (h *Orders) Place(c *gin.Context) {
	var req models.PlaceOrderRequest
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	o := &models.Order{ID: newID(), CustomerName: req.CustomerName, Status: models.StatusPending, Lines: req.Lines, CreatedAt: time.Now()}
	h.store.SaveOrder(o)
	c.JSON(http.StatusCreated, o)
}

func (h *Orders) ChangeStatus(c *gin.Context) {
	var change models.StatusChange
	if err := c.ShouldBindJSON(&change); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	o, ok := h.store.Order(c.Param("id"))
	if !ok {
		c.JSON(http.StatusNotFound, gin.H{"error": "not found"})
		return
	}
	o.Status = change.Status
	c.JSON(http.StatusOK, o)
}

func newID() string {
	return time.Now().Format("20060102150405.000000")
}
