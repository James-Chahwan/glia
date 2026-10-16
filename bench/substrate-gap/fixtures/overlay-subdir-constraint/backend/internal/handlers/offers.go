package handlers

import "example.com/backend/internal/repository"

type OfferHandler struct {
	repo *repository.OfferRepository
}

func (h *OfferHandler) Get(id string) (*repository.Offer, error) {
	return h.repo.FindByID(id)
}
