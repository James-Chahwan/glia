package store

import (
	"sync"

	"github.com/example/shop/api/internal/models"
)

type Memory struct {
	mu     sync.Mutex
	orders map[string]*models.Order
}

func NewMemory() *Memory {
	return &Memory{orders: map[string]*models.Order{}}
}

func (m *Memory) Order(id string) (*models.Order, bool) {
	m.mu.Lock()
	defer m.mu.Unlock()
	o, ok := m.orders[id]
	return o, ok
}

func (m *Memory) SaveOrder(o *models.Order) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.orders[o.ID] = o
}
