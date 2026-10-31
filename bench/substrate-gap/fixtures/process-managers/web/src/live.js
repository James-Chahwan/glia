const http = require("http");

function start() {
  http.createServer((req, res) => res.end("ok")).listen(3001);
}

start();
