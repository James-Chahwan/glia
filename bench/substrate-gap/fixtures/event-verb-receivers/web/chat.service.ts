// A Redux-style store: dispatch sends an action object, not a named event.
export class ChatService {
  constructor(private store: ChatStore) {}

  joinRoom(roomId: string) {
    this.store.dispatch({ type: "ROOM_JOINING", roomId });
  }
}
