import Redis from "ioredis";

const client = new Redis();

export async function putSession(id: string, v: string) {
  await client.set(id, v);
}
