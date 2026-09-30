import { ServiceBusClient } from "@azure/service-bus";

const sb = new ServiceBusClient(process.env.SB_CONN as string);

export async function drain() {
  const receiver = sb.createReceiver("orders");
  return receiver.receiveMessages(10);
}
