package services

import reg "example.com/shop/tokens"

// Ledger's field is typed through this file's `reg`, which is tokens.
type Ledger struct{ pairs *reg.Registry }
