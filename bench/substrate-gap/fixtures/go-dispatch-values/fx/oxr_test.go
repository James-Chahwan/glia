package fx

import "testing"

func TestOXR(t *testing.T) {
	src := func(id string) *OXR {
		o := NewOXR(id)
		return o
	}
	t.Run("free plan", func(t *testing.T) {
		o := src("app")
		if o.Provider() == "" {
			t.Fatal("no provider")
		}
	})
}

// TestMixed declares `o` twice in one fn scope with two types: the call is
// OXR's, and a first- or last-declaration guess would bind Other's.
func TestMixed(t *testing.T) {
	o := NewOXR("x")
	_ = o.Provider()
	t.Run("other", func(t *testing.T) {
		o := NewOther()
		_ = o
	})
}
