package apperr_test

import (
	"testing"

	"example.com/shop/services"
)

func TestRefusalText(t *testing.T) {
	if got := (&services.Refusal{Reason: "pep"}).Error(); got != "not available" {
		t.Fatalf("got %q", got)
	}
	_ = (&services.Refusal{Reason: "pep"}).Text()
}
