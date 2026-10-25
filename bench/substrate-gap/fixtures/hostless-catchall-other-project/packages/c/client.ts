export async function loadShell() {
  const res = await fetch('/*');
  return res.text();
}
