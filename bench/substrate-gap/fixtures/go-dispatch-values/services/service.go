package services

import (
	"example.com/shop/docstore"
	"example.com/shop/screening"
	"example.com/shop/tokens"
)

type SelfCheck interface {
	Name() string
	Check(id string) error
}

type kycCheck struct{}

func (kycCheck) Name() string          { return "kyc" }
func (kycCheck) Check(id string) error { return nil }

type reviewCheck struct{}

func (reviewCheck) Name() string          { return "review" }
func (reviewCheck) Check(id string) error { return nil }

// Labeler is ranged by nothing: its implementation stays dead.
type Labeler interface{ Label() string }

type tag struct{}

func (tag) Label() string { return "t" }

type Service struct {
	store  docstore.Store
	tokens *tokens.Registry
	lists  *screening.Registry
	checks []SelfCheck
	byName map[string]SelfCheck
	labels []Labeler
}

func New() *Service {
	return &Service{checks: []SelfCheck{kycCheck{}}, byName: map[string]SelfCheck{"review": reviewCheck{}}}
}

func (s *Service) Upload(key string) error { return s.store.Put(key) }

func (s *Service) Create(pair string) bool { return s.tokens.ActivePair(pair) && s.lists.Get(pair) }

func (s *Service) Reasons(id string) []string {
	var out []string
	for _, c := range s.checks {
		if err := c.Check(id); err != nil {
			out = append(out, c.Name())
		}
	}
	for i := range s.labels {
		_ = i
	}
	return out
}

func (s *Service) Lookup(name string) string {
	for _, c := range s.byName {
		return c.Name()
	}
	return ""
}

func (s *Service) OnDone(id string) {}

// OnFail is passed by nothing.
func (s *Service) OnFail(id string) {}
