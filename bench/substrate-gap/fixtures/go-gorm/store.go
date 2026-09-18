package store

import "gorm.io/gorm"

// ListUsers names the model, never the table: the TableName() override lives
// in model.go, so the table only reaches this query through the model-keyed
// entity both files share.
func ListUsers(db *gorm.DB) ([]User, error) {
	var us []User
	err := db.Model(&User{}).Find(&us).Error
	return us, err
}
