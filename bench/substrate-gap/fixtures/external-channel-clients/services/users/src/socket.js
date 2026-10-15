const { WebSocketServer } = require("ws");
const wss = new WebSocketServer({ path: "/ws", port: 8080 });
wss.on("connection", (s) => s.send("hi"));
const feed = new WebSocketServer({ path: "/feed", port: 8081 });
feed.on("connection", (s) => s.send("f"));
