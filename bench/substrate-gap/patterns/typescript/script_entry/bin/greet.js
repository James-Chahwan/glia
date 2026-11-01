#!/usr/bin/env node
// A shell script with no guard: run as `greet.js <name>`.
const { shout } = require("../lib/shout.cjs");

function greet(name) {
  return shout(`hello ${name}`);
}

console.log(greet(process.argv[2] ?? "world"));
