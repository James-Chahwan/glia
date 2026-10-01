package rootpkg

func WithTimeout(n int) int { return n }

// Closer is a one-method interface of the root package.
type Closer interface {
	Close() error
}
