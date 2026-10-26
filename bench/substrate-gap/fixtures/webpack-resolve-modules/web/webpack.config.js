const path = require('path');

const PATHS = {
  src: path.join(__dirname, 'src'),
  modules: path.join(__dirname, 'node_modules'),
};

module.exports = {
  entry: './src/page.js',
  resolve: {
    modules: [PATHS.src, 'node_modules', PATHS.modules],
  },
};
