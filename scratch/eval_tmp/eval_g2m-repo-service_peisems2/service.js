const repo = require('./repo');
exports.list = function () { const r = repo.getItems(); return { items: r }; };  // BUG: wraps in object
exports.create = function (name) { repo.addItem(name); };
