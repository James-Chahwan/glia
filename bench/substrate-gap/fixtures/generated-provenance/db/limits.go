// Limits. Do not edit without updating the migration.
package db

const MaxUsers = 10

func Limit() int {
	return MaxUsers
}
