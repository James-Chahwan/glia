package main

import (
	"github.com/go-chi/chi/v5"

	"example.com/h/svc"
)

type server struct{ handlers *svc.Handlers }

func routes(r chi.Router, s *svc.Svc) {
	h := s.Handlers()
	r.Get("/list", h.List)
}

func (s *server) mount(r chi.Router) {
	h := s.handlers
	r.Post("/items", h.Create)
}
