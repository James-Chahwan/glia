// Not GraphQL: node-postgres, mqtt.js and TanStack Query share the needle words.
import { Client } from "pg";
import mqtt from "mqtt";
import { useQuery } from "@tanstack/react-query";

export async function countRows(client: Client) {
  const res = await client.query("SELECT count(*) FROM orders");
  return res.rows[0];
}

export function watchSensors(client: mqtt.MqttClient) {
  client.subscribe("sensors/temp");
}

export function useTodos() {
  return useQuery({ queryKey: ["todos"], queryFn: () => fetch("/api/todos") });
}
