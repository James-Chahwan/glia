#!/usr/bin/env node
// An ES-module script run from a shell: `hello.mjs <name>`.
function hello(name) {
  return `hello ${name}`;
}

console.log(hello(process.argv[2] ?? 'world'));
