// A CommonJS helper a script requires by its `.cjs` name.
function shout(text) {
  return text.toUpperCase();
}

module.exports = { shout };
