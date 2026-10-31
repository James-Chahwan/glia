package wsdemo

import (
	"net/http"
	"testing"
)

func TestSubprotocols(t *testing.T) {
	r := &http.Request{}
	if got := Subprotocols(r); got != nil {
		t.Fatalf("got %v", got)
	}
}
