package ws

import (
	"encoding/json"
	"testing"
)

func TestJSON(t *testing.T) {
	if _, err := json.Marshal(1); err != nil {
		t.Fatal(err)
	}
}
