// A function-local lower-case `topic` in ANOTHER file. The repo const table
// binds it, but a cross-file lookup trusts only constant-shaped keys, so
// publishTo(topic) in publish.ts must not borrow this value.
export function auditLabel(): string {
  const topic = 'audit-log';
  return `audit:${topic}`;
}
