import * as signalR from "@microsoft/signalr";

const connection = new signalR.HubConnectionBuilder().withUrl("/hubs/chat").build();

export async function send(user: string, message: string): Promise<void> {
  await connection.invoke("SendMessage", user, message);
}
