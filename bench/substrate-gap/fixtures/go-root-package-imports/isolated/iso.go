package isolated

// file has a Close() but no import path links it to the root package.
type file struct{}

func (file) Close() error { return nil }
