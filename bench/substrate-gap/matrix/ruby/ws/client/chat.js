import { createConsumer } from "@rails/actioncable"

const consumer = createConsumer()

export function joinChat(room) {
  return consumer.subscriptions.create({ channel: "ChatChannel", room: room }, {
    received(data) {
      console.log(data)
    }
  })
}
