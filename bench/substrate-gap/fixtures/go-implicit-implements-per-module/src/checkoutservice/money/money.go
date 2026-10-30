package money

type Formatter interface {
	Format(units int64) string
	Currency() string
}
