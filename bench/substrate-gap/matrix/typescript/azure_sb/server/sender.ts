import { ServiceBusClient } from "@azure/service-bus";

const sb = new ServiceBusClient(process.env.SB_CONN as string);

export async function send(body: string) {
  const sender = sb.createSender("orders");
  await sender.sendMessages({ body });
}
