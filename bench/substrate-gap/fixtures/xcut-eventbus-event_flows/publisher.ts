import { EventEmitter } from "events";

export const bus = new EventEmitter();

export function createUser(name: string) {
  const user = { id: 1, name };
  bus.emit("userCreated", user);
  return user;
}
