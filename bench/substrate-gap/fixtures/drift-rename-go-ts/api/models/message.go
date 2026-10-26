package models

type Message struct {
	MessageID string `json:"messageUid"`
	Subject   string `json:"subject"`
	SentAt    string `json:"sentAt"`
}
