#!/usr/bin/env node
// A shell script: run as `count-lines.js <file>`.
const fs = require("fs");

function count(path) {
  return fs.readFileSync(path, "utf8").split("\n").length;
}

if (require.main === module) {
  console.log(count(process.argv[2]));
}
