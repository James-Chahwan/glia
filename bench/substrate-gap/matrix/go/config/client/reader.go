package main

import (
	"database/sql"
	"os"
)

func Open() (*sql.DB, error) {
	return sql.Open("postgres", os.Getenv("DATABASE_URL"))
}
