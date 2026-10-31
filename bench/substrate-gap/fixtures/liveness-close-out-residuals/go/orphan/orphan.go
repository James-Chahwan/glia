package orphan

// init never runs: no package imports orphan.
func init() { setup() }

func setup() {}
