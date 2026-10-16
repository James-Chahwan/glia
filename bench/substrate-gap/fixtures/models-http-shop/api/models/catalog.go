package models

// Article is the stored row; it is never serialised.
type Article struct {
	ID         string `json:"id"`
	Title      string `json:"title"`
	PriceCents int64  `json:"priceCents"`
	Stock      int    `json:"stock"`
}

type CatalogEntry struct {
	ID         string `json:"id"`
	Title      string `json:"title"`
	PriceCents int64  `json:"priceCents"`
	Available  bool   `json:"available"`
}

type Listing struct {
	ID         string `json:"id"`
	Title      string `json:"title"`
	PriceCents int64  `json:"priceCents"`
}

type ListingWithSeller struct {
	Listing
	SellerName string `json:"sellerName"`
}

type DailySummary struct {
	Total int    `json:"total"`
	Count int    `json:"count"`
	Day   string `json:"day"`
}

type WeeklySummary struct {
	Total int `json:"total"`
	Count int `json:"count"`
	Week  int `json:"week"`
}
