package main

import (
	"net/http"
	"sync"

	"golang.org/x/sync/errgroup"
)

type Repo struct{}

var (
	once sync.Once
	repo *Repo
)

func NewRepo() *Repo { return &Repo{} }

func worker()                           {}
func cleanup()                          {}
func flush() error                      { return nil }
func writeHealth(w http.ResponseWriter) {}
func parseToken(r *http.Request) bool   { return r != nil }

// Provider builds its repository inside sync.Once.Do (quokka repository_provider.go).
func Provider() *Repo {
	once.Do(func() {
		repo = NewRepo()
	})
	return repo
}

// Start runs work in a goroutine, a deferred closure and an errgroup.
func Start() error {
	go func() {
		worker()
	}()
	defer func() {
		cleanup()
	}()
	var g errgroup.Group
	g.Go(func() error {
		return flush()
	})
	return g.Wait()
}

// Auth returns a middleware closure (Kina JWTAuthMiddleware / quokka
// Middleware/jwt.go shape): the closure's calls belong to Auth.
func Auth(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if !parseToken(r) {
			return
		}
		next(w, r)
	}
}

// Routes registers a func-literal route handler: its callee is the route's
// HANDLED_BY target, never a CALLS of Routes.
func Routes() {
	http.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		writeHealth(w)
	})
}

func main() {
	Routes()
	_ = Start()
	_ = Provider()
	_ = Auth(nil)
}
