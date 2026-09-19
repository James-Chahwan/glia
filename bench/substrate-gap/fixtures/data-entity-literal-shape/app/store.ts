// DynamoDB SDK v3: a table named by the environment, then a literal (the control).
import { DynamoDBClient, PutItemCommand } from "@aws-sdk/client-dynamodb";

const ddb = new DynamoDBClient({});

export async function saveOrder(id: string) {
  await ddb.send(new PutItemCommand({
    TableName: process.env.ORDERS_TABLE,
    Item: { id: { S: id }, status: { S: "pending" } },
  }));
}

export const sessionPut = { TableName: "sessions", Item: {} };
