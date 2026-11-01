// A webpack plugin (a CommonJS `.cjs` module): webpack calls apply(compiler),
// which taps the compiler's and each compilation's hooks.
'use strict';

function checkAssets(assets) {
  return Object.keys(assets).length;
}

class ReportPlugin {
  apply(compiler) {
    compiler.hooks.done.tap('ReportPlugin', (stats) => {
      this.report(stats);
    });
    compiler.hooks.emit.tapAsync('ReportPlugin', this.onEmit.bind(this));
    compiler.hooks.compilation.tap('ReportPlugin', (compilation) => {
      compilation.hooks.processAssets.tap('ReportPlugin', checkAssets);
    });
  }

  onEmit(compilation, callback) {
    callback();
  }

  report(stats) {
    return stats.toString();
  }
}

module.exports = ReportPlugin;
