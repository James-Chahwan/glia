package tests

import "testing"

// Closable lives in a test file: only this directory's test code sees it.
type Closable interface {
	Close()
}

type fakeConn struct{}

func (f fakeConn) Close() {}

func TestClose(t *testing.T) {
	var c Closable = fakeConn{}
	c.Close()
}
