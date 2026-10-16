package models

import "time"

type Audit struct {
	CreatedAt time.Time  `json:"createdAt"`
	UpdatedAt *time.Time `json:"updatedAt,omitempty"`
}

type Account struct {
	Audit
	ID          string   `json:"id"`
	DisplayName string   `json:"display_name" binding:"required"`
	Email       *string  `json:"email,omitempty"`
	Roles       []string `json:"roles"`
	Owner       Person   `json:"owner"`
	Password    string   `json:"-"`
	Dash        string   `json:"-,"`
	Balance     int64    `json:"balance,string"`
	Region      string
	secret      string
}

type Person struct {
	Name string `json:"name"`
	Age  int    `json:"age,omitempty"`
}

type cache struct {
	rows map[string]*Account
}
