package repositories

import (
	"go.mongodb.org/mongo-driver/mongo"
)

// ChatPreview is one row of the chat list.
type ChatPreview struct {
	RoomID string
	Last   string
}

// ChatPreviewRepository reads and writes chat previews.
type ChatPreviewRepository struct {
	collection *Collection[ChatPreview]
}

// NewChatPreviewRepository wires the repository to its collection.
func NewChatPreviewRepository(client *mongo.Client, database string) *ChatPreviewRepository {
	// NewCollection[ChatPreview](client, database, "legacy_previews") was the old name.
	return &ChatPreviewRepository{collection: NewCollection[ChatPreview](client, database, "chat_previews")}
}
