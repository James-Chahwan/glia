import { createClient } from "@connectrpc/connect";
import { createConnectTransport } from "@connectrpc/connect-web";
import { ElizaService } from "./gen/eliza_pb";

const transport = createConnectTransport({ baseUrl: "http://localhost:8080" });
const client = createClient(ElizaService, transport);

export async function talk(sentence: string) {
  const res = await client.say({ sentence });
  return res.sentence;
}
