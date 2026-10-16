package models

type Member struct {
	MemberID    string   `json:"memberId"`
	Handle      string   `json:"handle"`
	DisplayName string   `json:"displayName"`
	Badges      []string `json:"badges"`
	Email       string   `json:"-"`
}
