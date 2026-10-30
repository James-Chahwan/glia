package repo

import "database/sql"

func Open(dsn string) (*sql.DB, error) {
	return sql.Open("postgres", dsn)
}
