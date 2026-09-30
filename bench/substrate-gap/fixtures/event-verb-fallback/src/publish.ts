import { EventBridge } from "aws-sdk";

const eventBridge = new EventBridge();

export async function announce(id: string) {
  await eventBridge.putEvents({ Entries: [{ Source: "shop.orders", DetailType: "OrderPlaced", Detail: JSON.stringify({ id }) }] }).promise();
}
