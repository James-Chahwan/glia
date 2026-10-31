package used

var ready bool

// init runs: main imports this package.
func init() { prepare() }

func prepare() { ready = true }

func Run() {}
