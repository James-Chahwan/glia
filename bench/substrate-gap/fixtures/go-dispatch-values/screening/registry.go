package screening

type Registry struct{ lists map[string]bool }

func (r *Registry) Get(name string) bool { return r.lists[name] }

// ActivePair shares tokens.Registry's method name and is called by nothing.
func (r *Registry) ActivePair(p string) bool { return false }
