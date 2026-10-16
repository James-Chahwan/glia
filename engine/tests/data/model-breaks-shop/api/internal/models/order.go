package models

import "time"

type OrderStatus string

const (
	StatusPending   OrderStatus = "pending"
	StatusPaid      OrderStatus = "paid"
	StatusShipped   OrderStatus = "shipped"
	StatusCancelled OrderStatus = "cancelled"
)

type OrderLine struct {
	ProductID      string `json:"productId"`
	Quantity       int    `json:"quantity"`
	UnitPriceCents int64  `json:"unitPriceCents"`
}

type Order struct {
	ID           string      `json:"id"`
	CustomerName string      `json:"customerName"`
	Status       OrderStatus `json:"status"`
	Lines        []OrderLine `json:"lines"`
	TotalCents   int64       `json:"totalCents"`
	CreatedAt    time.Time   `json:"createdAt"`
	InternalNote string      `json:"-"`
}

type PlaceOrderRequest struct {
	CustomerName string      `json:"customerName" binding:"required"`
	Lines        []OrderLine `json:"lines" binding:"required"`
}

type StatusChange struct {
	Status OrderStatus `json:"status" binding:"required"`
}
