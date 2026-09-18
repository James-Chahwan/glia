package store

// User is a GORM model. This file imports nothing: the `gorm:"..."` struct
// tags are its only GORM evidence, and the TableName() override below is the
// only place the real table name appears.
type User struct {
	ID    uint   `gorm:"primaryKey"`
	Email string `gorm:"uniqueIndex"`
}

func (User) TableName() string { return "app_users" }
