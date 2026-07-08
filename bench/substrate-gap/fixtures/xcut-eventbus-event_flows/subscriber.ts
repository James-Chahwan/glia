import { bus } from "./publisher";

export function registerHandlers() {
  bus.on("userCreated", (user) => {
    sendWelcomeEmail(user);
  });
}

function sendWelcomeEmail(user: { name: string }) {
  console.log(`welcome ${user.name}`);
}
