package indexer

type Indexer struct{ done func(string) }

// defaultDone is initialised to a func value: a USES of logDone.
var defaultDone = logDone

func logDone(id string) {}

func New() *Indexer { return &Indexer{done: defaultDone} }

func (i *Indexer) SetOnCompleted(f func(string)) { i.done = f }

func (i *Indexer) Run() { i.done("trade-1") }
