package conn

import (
	"github.com/demo/checkoutservice/money"
)

var _ money.Formatter = (*Price)(nil)

type Price struct{ code string }

func (p *Price) Format(units int64) string { return p.code }

func (p *Price) Currency() string { return p.code }
