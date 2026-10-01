package wrap

import rp "example.com/rootpkg"

func Dial() *rp.Client { return rp.NewClient("y") }
