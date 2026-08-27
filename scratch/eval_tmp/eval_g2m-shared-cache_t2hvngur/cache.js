const store = new Map();
exports.get = function (key) { return store.get(key); };
exports.set = function (key, value) { store.set(key, value); };
