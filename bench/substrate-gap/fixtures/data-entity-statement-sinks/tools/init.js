const fs = require("fs");

function init() {
  fs.writeFileSync("db/init.sql", "CREATE TABLE pallets (id INT);\n");
}

module.exports = { init };
