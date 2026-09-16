// TS fetch client for the minimal-API routes above. Client-endpoint extraction
// is already proven for TS, so a 0.00 on HTTP_CALLS means the C# side emitted
// no ROUTE at all — which is exactly the top-level `global_statement` blind
// spot this fixture pins.
export async function health(): Promise<unknown> {
  return fetch("/health");
}

export async function createOrder(body: unknown): Promise<unknown> {
  return fetch("/orders", { method: "POST", body: JSON.stringify(body) });
}
