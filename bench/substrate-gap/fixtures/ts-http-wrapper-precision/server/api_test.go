package main

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

// PerformRequest drives the app's own router in-process: a test helper, not
// an HTTP client of another service.
func PerformRequest(r http.Handler, method, path string) *httptest.ResponseRecorder {
	req, _ := http.NewRequest(method, path, nil)
	w := httptest.NewRecorder()
	r.ServeHTTP(w, req)
	return w
}

func TestStats(t *testing.T) {
	r := setupRouter()
	PerformRequest(r, "GET", "/api/stats")
}
