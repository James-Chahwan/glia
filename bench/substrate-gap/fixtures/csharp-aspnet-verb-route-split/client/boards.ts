// The GET call must pair with BoardsController's GET route, never with the
// PUT / DELETE template one slash longer.
export async function loadBoard(name: string): Promise<unknown> {
  const r = await fetch(`/boards/${name}`, { method: "GET" });
  return r.json();
}
