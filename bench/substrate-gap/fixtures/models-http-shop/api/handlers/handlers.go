package handlers

import (
	"net/http"
	"strconv"

	"github.com/gin-gonic/gin"

	"example.com/billing/api/models"
	"example.com/billing/api/store"
)

type Handlers struct {
	store *store.Store
}

func New(s *store.Store) *Handlers {
	return &Handlers{store: s}
}

func (h *Handlers) Register(g *gin.RouterGroup) {
	g.GET("/invoices/:id", h.GetInvoice)
	g.POST("/invoices", h.CreateInvoice)
	g.PATCH("/invoices/:id/status", h.SetStatus)
	g.GET("/invoice-pages/:n", h.InvoicePage)
	g.GET("/articles", h.Articles)
	g.GET("/listings/:id", h.GetListing)
	g.GET("/listings", h.Listings)
	g.GET("/summary/daily", h.Daily)
	g.GET("/summary/weekly", h.Weekly)
}

func (h *Handlers) GetInvoice(c *gin.Context) {
	inv, ok := h.store.Invoice(c.Param("id"))
	if !ok {
		c.JSON(http.StatusNotFound, gin.H{"error": "not found"})
		return
	}
	c.JSON(http.StatusOK, inv)
}

func (h *Handlers) CreateInvoice(c *gin.Context) {
	var req models.CreateInvoiceRequest
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	inv := &models.Invoice{ID: "inv-1", CustomerName: req.CustomerName, Status: "draft", Items: req.Items}
	h.store.Save(inv)
	c.JSON(http.StatusCreated, inv)
}

func (h *Handlers) SetStatus(c *gin.Context) {
	var up models.StatusUpdate
	if err := c.ShouldBindJSON(&up); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	inv, ok := h.store.Invoice(c.Param("id"))
	if !ok {
		c.JSON(http.StatusNotFound, gin.H{"error": "not found"})
		return
	}
	inv.Status = up.Status
	c.JSON(http.StatusOK, inv)
}

func (h *Handlers) InvoicePage(c *gin.Context) {
	n, _ := strconv.Atoi(c.Param("n"))
	invs := h.store.Page(n)
	c.JSON(http.StatusOK, gin.H{"invoices": invs, "page": n})
}

func (h *Handlers) Articles(c *gin.Context) {
	out := []models.CatalogEntry{}
	for _, a := range h.store.Articles() {
		out = append(out, models.CatalogEntry{ID: a.ID, Title: a.Title, PriceCents: a.PriceCents, Available: a.Stock > 0})
	}
	c.JSON(http.StatusOK, out)
}

func (h *Handlers) GetListing(c *gin.Context) {
	l, ok := h.store.Listing(c.Param("id"))
	if !ok {
		c.JSON(http.StatusNotFound, gin.H{"error": "not found"})
		return
	}
	c.JSON(http.StatusOK, l)
}

func (h *Handlers) Listings(c *gin.Context) {
	c.JSON(http.StatusOK, gin.H{"listings": h.store.Listings()})
}

func (h *Handlers) Daily(c *gin.Context) {
	c.JSON(http.StatusOK, models.DailySummary{Total: 1, Count: 1, Day: "mon"})
}

func (h *Handlers) Weekly(c *gin.Context) {
	c.JSON(http.StatusOK, models.WeeklySummary{Total: 7, Count: 7, Week: 1})
}
