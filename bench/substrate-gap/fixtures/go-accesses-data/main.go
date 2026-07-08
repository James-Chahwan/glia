package main

import (
	"database/sql"

	_ "github.com/lib/pq"
)

func getUsers(db *sql.DB) error {
	rows, err := db.Query("SELECT id, name FROM users WHERE active = true")
	if err != nil {
		return err
	}
	defer rows.Close()
	return nil
}

func main() {
	db, _ := sql.Open("postgres", "postgres://localhost/app")
	getUsers(db)
}
