package stock

// Item is a warehouse row. It is never returned to a client: the resolvers
// map it onto model.Item.
type Item struct {
	ID         string `json:"id"`
	Label      string `json:"label"`
	PriceCents int64  `json:"priceCents"`
	OnHand     int    `json:"onHand"`
}

type Warehouse struct {
	rows map[string]*Item
}

func (w *Warehouse) All() []*Item {
	out := make([]*Item, 0, len(w.rows))
	for _, it := range w.rows {
		out = append(out, it)
	}
	return out
}

func (w *Warehouse) Adjust(id string, delta int) *Item {
	it := w.rows[id]
	if it != nil {
		it.OnHand += delta
	}
	return it
}
