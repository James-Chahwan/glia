package svc

import "net/http"

type Svc struct{}
type Handlers struct{}
type Other struct{}

func (s *Svc) Handlers() *Handlers { return &Handlers{} }

func (h *Handlers) List(w http.ResponseWriter, r *http.Request)   {}
func (h *Handlers) Create(w http.ResponseWriter, r *http.Request) {}
func (o *Other) List(w http.ResponseWriter, r *http.Request)      {}
func (o *Other) Create(w http.ResponseWriter, r *http.Request)    {}
