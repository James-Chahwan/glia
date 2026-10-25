// The web app's SDK: one function per backend call.
export async function getAlbums(): Promise<unknown> {
  const res = await fetch("/api/albums");
  return res.json();
}
