package auth

// Check validates a token.
func Check(token string) bool {
	return token != ""
}
