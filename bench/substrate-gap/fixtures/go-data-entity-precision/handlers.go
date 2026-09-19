package handlers

import (
	"context"
	"database/sql"
	"fmt"
	"net/http"

	"github.com/gin-gonic/gin"
	_ "github.com/lib/pq"
	"go.mongodb.org/mongo-driver/mongo"
)

// Package-level statements: passed to db calls by name, so only the
// cross-cutting data-entity scan sees them (the Go parser reads call-argument
// literals only). Results are copied from the pool before they are returned.
const scoresSQL = `WITH recent AS (SELECT id, tags FROM orders WHERE created_at > now() - interval '7 days')
SELECT r.id, COALESCE(s.total, 0), EXTRACT(EPOCH FROM COALESCE(s.paid_at, now()))
FROM recent r
LEFT JOIN LATERAL (SELECT SUM(amount) AS total, MAX(paid_at) AS paid_at FROM payments p WHERE p.order_id = r.id) s ON true
CROSS JOIN LATERAL jsonb_array_elements(r.tags) t`

const upsertPrefsSQL = `INSERT INTO prefs (user_id, theme) VALUES ($1, $2)
ON CONFLICT (user_id) DO UPDATE SET theme = EXCLUDED.theme`

const tagsSQL = "SELECT t.value FROM jsonb_array_elements($1::jsonb) t"

const errDeleteSpace = "delete from spaces key=%q: %w"

func ListUsers(db *sql.DB) (*sql.Rows, error) {
	return db.Query("SELECT id, name FROM users WHERE active = true")
}

func RecentOrders(db *sql.DB, since string) (*sql.Rows, error) {
	q := "SELECT o.id, i.sku " +
		"FROM orders o JOIN order_items i ON i.order_id = o.id " +
		"WHERE o.created_at > $1"
	return db.Query(q, since)
}

func Scores(db *sql.DB) (*sql.Rows, error) { return db.Query(scoresSQL) }

func Upsert(db *sql.DB, user, theme string) error {
	_, err := db.Exec(upsertPrefsSQL, user, theme)
	return err
}

func Tags(db *sql.DB, raw string) (*sql.Rows, error) { return db.Query(tagsSQL, raw) }

func Fanout(done <-chan struct{}, in <-chan int, out chan<- int) {
	for {
		select {
		case v := <-in:
			out <- v
		case <-done:
			return
		}
	}
}

func Me(c *gin.Context) {
	c.JSON(http.StatusUnauthorized, gin.H{"error": "Failed to extract email from token"})
	c.String(http.StatusOK, "You've been unsubscribed from all emails.")
}

func Remove(key string, err error) error { return fmt.Errorf(errDeleteSpace, key, err) }

func Events(ctx context.Context, client *mongo.Client, ev any) error {
	_, err := client.Database("shop").Collection("events").InsertOne(ctx, ev)
	return err
}
