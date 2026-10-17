package services

// Refusal is an error a handler writes neutrally.
type Refusal struct{ Reason string }

func (r *Refusal) Error() string { return "not available" }

// Text shares its name with apperr.Refusal's Text.
func (r *Refusal) Text() string { return r.Reason }

// Detail is called by nothing.
func (r *Refusal) Detail() string { return r.Reason }
