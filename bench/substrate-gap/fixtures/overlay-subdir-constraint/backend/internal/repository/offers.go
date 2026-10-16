package repository

type Offer struct {
	ID string
}

type OfferRepository struct{}

func (r *OfferRepository) FindByID(id string) (*Offer, error) {
	return &Offer{ID: id}, nil
}
