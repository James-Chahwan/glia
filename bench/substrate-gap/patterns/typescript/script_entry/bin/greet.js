#!/usr/bin/env node
// A shell script with no guard: run as `greet.js <name>`.
function greet(name) {
  return `hello ${name}`;
}

console.log(greet(process.argv[2] ?? "world"));
