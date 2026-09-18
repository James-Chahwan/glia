// AWS SDK v3 command objects are requests to a service client, not events.
import { DynamoDBClient, PutItemCommand } from "@aws-sdk/client-dynamodb";

const ddb = new DynamoDBClient({});

export async function saveOrder(id: string) {
  await ddb.send(new PutItemCommand({ TableName: "orders", Item: { id: { S: id } } }));
}
