export async function loadX() {
  const res = await fetch('/x');
  return res.json();
}

export async function loadEverything() {
  const res = await fetch('/*');
  return res.json();
}
