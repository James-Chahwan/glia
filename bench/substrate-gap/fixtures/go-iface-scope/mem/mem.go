package mem

import "example.com/scope/store"

type Mem struct{}

func (m *Mem) Get(key string) string { return key }
func (m *Mem) Put(key, value string) {}
func (m *Mem) Close()                {}

var _ store.Store = (*Mem)(nil)
