package store

import "example.com/fleet/models"

type Store struct {
	vehicles map[string]*models.Vehicle
	trips    []models.Trip
}

func (s *Store) Vehicle(id string) (*models.Vehicle, bool) {
	v, ok := s.vehicles[id]
	return v, ok
}

func (s *Store) Vehicles() []*models.Vehicle {
	out := make([]*models.Vehicle, 0, len(s.vehicles))
	for _, v := range s.vehicles {
		out = append(out, v)
	}
	return out
}

func (s *Store) Trips(vehicleID string) []models.Trip {
	var out []models.Trip
	for _, t := range s.trips {
		if t.VehicleID == vehicleID {
			out = append(out, t)
		}
	}
	return out
}
