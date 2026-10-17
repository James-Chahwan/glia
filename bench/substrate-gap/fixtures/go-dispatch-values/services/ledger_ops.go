package services

import reg "example.com/shop/screening"

// Pairs sits in a file whose `reg` is screening: the field is still a
// tokens.Registry, read through the declaring file's import.
func (l *Ledger) Pairs(p string) bool { return l.pairs.ActivePair(p) }

var _ *reg.Registry
