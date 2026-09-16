// TS fetch client for the composed ASP.NET routes above. Client-endpoint
// extraction is already proven for TS; this side is the pairing partner, so a
// 0.00 here means the C# ROUTE qname never became absolute.
export async function loadOrder(id: string): Promise<unknown> {
  const r = await fetch(`/api/v2/orders/${id}`);
  return r.json();
}

export async function createOrder(body: unknown): Promise<unknown> {
  return fetch("/api/v2/orders", { method: "POST", body: JSON.stringify(body) });
}
