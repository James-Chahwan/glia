package api

// Validate reports whether id is a usable user id.
func Validate(id string) bool {
	return len(id) > 0
}
