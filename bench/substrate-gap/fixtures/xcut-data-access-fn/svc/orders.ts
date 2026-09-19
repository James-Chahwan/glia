import { Pool } from 'pg';

const pool = new Pool();

// Module scope: the statement is built here and run by audit() below, so the
// access site is the module, not a function.
const AUDIT_SQL = 'SELECT id FROM audit_log';

export async function saveOrder(order) {
  await pool.query('INSERT INTO orders (id) VALUES ($1)', [order.id]);
}

export async function loadOrder(id) {
  return pool.query('SELECT * FROM orders WHERE id = $1', [id]);
}

export async function archiveOrder(id) {
  const row = await pool.query('SELECT * FROM orders WHERE id = $1', [id]);
  await pool.query('DELETE FROM orders WHERE id = $1', [id]);
  return row;
}

export async function audit() {
  return pool.query(AUDIT_SQL);
}
