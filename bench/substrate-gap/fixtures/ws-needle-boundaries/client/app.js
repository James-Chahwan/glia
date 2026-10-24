function io(label) {
  return label;
}

const pairs = rows.windows(3);
io(pairs);
const s = new WebSocket('wss://chat.example/chat');
