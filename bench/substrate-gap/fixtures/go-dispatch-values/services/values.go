package services

// handle is a package func that only shadowed names refer to: it stays dead.
func handle(id string) {}

func register(f func(string)) {}

func use(s string) {}

// Wrap's named result `handle` shadows the package func.
func Wrap() (handle func(string)) {
	register(handle)
	return
}

// Pick's local const `handle` shadows it too.
func Pick() {
	const handle = "x"
	use(handle)
}

// Topic and Rule share a method name; Topics ranges a map's keys only.
type Topic struct{}

func (Topic) Describe() string { return "topic" }

type Rule struct{}

func (Rule) Describe() string { return "rule" }

type Rules struct{ byTopic map[Topic]Rule }

// Topics' lone range name is the key (a Topic), never the value type Rule.
func (r *Rules) Topics() []string {
	var out []string
	for k := range r.byTopic {
		out = append(out, k.Describe())
	}
	return out
}
