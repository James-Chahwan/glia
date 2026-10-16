package store

import "example.com/billing/api/models"

type Store struct {
	invoices map[string]*models.Invoice
	articles []models.Article
	listings map[string]*models.Listing
}

func New() *Store {
	return &Store{invoices: map[string]*models.Invoice{}, listings: map[string]*models.Listing{}}
}

func (s *Store) Invoice(id string) (*models.Invoice, bool) {
	inv, ok := s.invoices[id]
	return inv, ok
}

func (s *Store) Page(n int) []models.Invoice {
	out := []models.Invoice{}
	for _, inv := range s.invoices {
		out = append(out, *inv)
	}
	return out
}

func (s *Store) Save(inv *models.Invoice) {
	s.invoices[inv.ID] = inv
}

func (s *Store) Articles() []models.Article {
	return s.articles
}

func (s *Store) Listing(id string) (*models.Listing, bool) {
	l, ok := s.listings[id]
	return l, ok
}

func (s *Store) Listings() []models.ListingWithSeller {
	out := []models.ListingWithSeller{}
	for _, l := range s.listings {
		out = append(out, models.ListingWithSeller{Listing: *l, SellerName: "shop"})
	}
	return out
}
