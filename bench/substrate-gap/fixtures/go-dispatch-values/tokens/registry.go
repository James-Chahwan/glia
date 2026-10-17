package tokens

type Registry struct{ pairs map[string]bool }

func (r *Registry) ActivePair(p string) bool { return r.pairs[p] }

// HasPair is called by nothing.
func (r *Registry) HasPair(p string) bool { return r.pairs[p] }
