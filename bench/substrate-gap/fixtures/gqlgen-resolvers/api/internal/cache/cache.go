package cache

// cacheResolver resolves cache keys; it is no gqlgen resolver (no *Resolver embed).
type cacheResolver struct{}

func (c *cacheResolver) Products(key string) []string { return nil }
