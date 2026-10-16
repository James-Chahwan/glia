package models

import "time"

type InvoiceItem struct {
	SKU       string `json:"sku"`
	Qty       int    `json:"qty"`
	UnitCents int64  `json:"unitCents"`
}

type Invoice struct {
	ID           string        `json:"id"`
	CustomerName string        `json:"customer_name"`
	Status       string        `json:"status"`
	Items        []InvoiceItem `json:"items"`
	TotalCents   int64         `json:"totalCents"`
	IssuedAt     time.Time     `json:"issuedAt"`
	Note         string        `json:"-"`
}

type CreateInvoiceRequest struct {
	CustomerName string        `json:"customerName" binding:"required"`
	Items        []InvoiceItem `json:"items" binding:"required"`
}

type StatusUpdate struct {
	Status string `json:"status" binding:"required"`
}
