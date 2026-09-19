package store

import (
	"context"
	"database/sql"
	"fmt"
)

// Every statement below is a call argument inside a function: the shapes the
// Go parser's own per-call-argument SQL scan read as tables before LE.4a
// (quokka-stack sql:spaces, lapse's CTE aliases and SET, Kina's sql:a).

func Remove(key string, err error) error {
	return fmt.Errorf("delete from spaces key=%q: %w", key, err)
}

func Upsert(ctx context.Context, db *sql.DB, user, theme string) error {
	_, err := db.ExecContext(ctx, `INSERT INTO prefs (user_id, theme) VALUES ($1, $2)
		ON CONFLICT (user_id) DO UPDATE SET theme = EXCLUDED.theme`, user, theme)
	return err
}

func RaiseStatus(ctx context.Context, db *sql.DB, user, status string) error {
	_, err := db.ExecContext(ctx, `
		INSERT INTO user_regimes (user_id, screening_status)
		VALUES ($1, $2)
		ON CONFLICT (user_id)
		DO UPDATE SET
		  -- two concurrent screens cannot interleave into a downgrade.
		  screening_status = EXCLUDED.screening_status`, user, status)
	return err
}

func List(ctx context.Context, db *sql.DB, org string) (*sql.Rows, error) {
	return db.QueryContext(ctx, `
		WITH opp_ids AS (
			SELECT DISTINCT opportunity_id FROM briefs WHERE organisation_id = $1
			UNION
			SELECT DISTINCT opportunity_id FROM pursuit_task WHERE organisation_id = $1
		),
		task_counts AS (
			SELECT opportunity_id, COUNT(*) AS total FROM pursuit_task GROUP BY opportunity_id
		),
		chosen_brief AS (
			SELECT DISTINCT ON (opportunity_id) id, opportunity_id FROM briefs
		)
		SELECT o.id FROM opp_ids
		JOIN opportunities o ON o.id = opp_ids.opportunity_id
		LEFT JOIN chosen_brief b ON b.opportunity_id = o.id
		LEFT JOIN task_counts tc ON tc.opportunity_id = o.id`, org)
}

func Get(ctx context.Context, db *sql.DB, org, opp string) *sql.Row {
	return db.QueryRowContext(ctx, `
		WITH tc AS (SELECT COUNT(*) AS total FROM pursuit_task WHERE opportunity_id = $2),
		cb AS (SELECT id FROM briefs WHERE opportunity_id = $2 LIMIT 1)
		SELECT o.id FROM opportunities o
		LEFT JOIN cb ON true
		LEFT JOIN tc ON true
		WHERE o.id = $2`, org, opp)
}

func Outcomes(ctx context.Context, db *sql.DB, org string) (*sql.Rows, error) {
	return db.QueryContext(ctx,
		`WITH
		 response_outcomes AS (
		   SELECT o.id FROM outcomes o JOIN opportunities op ON op.id = o.opportunity_id
		 ),
		 pursuit_outcomes AS (
		   -- one row per (org, opp, result) regardless of how many briefs.
		   SELECT b.id FROM briefs b WHERE b.organisation_id = $1
		 )
		 SELECT id FROM response_outcomes
		 UNION ALL
		 SELECT id FROM pursuit_outcomes`, org)
}
