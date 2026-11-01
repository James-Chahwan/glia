// A Deno / Bun script: `deno run scripts/seed.ts`. No shebang; the guard
// runs it only when it is the entry module.
export async function seed(rows: number): Promise<number> {
  return rows * 2;
}

async function main() {
  console.log(await seed(10));
}

if (import.meta.main) {
  await main();
}
