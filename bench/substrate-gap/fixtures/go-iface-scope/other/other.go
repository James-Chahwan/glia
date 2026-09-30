package other

// File has a Close() but its package neither imports store nor is imported
// by it: it can never be used as a store.Closer.
type File struct{}

func (f *File) Close() {}

// Wrong has Store's method NAMES with other signatures.
type Wrong struct{}

func (w *Wrong) Get(key int) int    { return key }
func (w *Wrong) Put(key, value int) {}
