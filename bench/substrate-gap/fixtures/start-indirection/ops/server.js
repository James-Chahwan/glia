const http = require("http");

function listen(port) {
  http.createServer((_req, res) => res.end("ops")).listen(port);
}

listen(9090);
