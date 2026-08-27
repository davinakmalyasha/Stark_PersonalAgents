const items = [{ id: 1, name: 'apple' }];
exports.getItems = function () { return items; };
exports.addItem = function (name) { items.push({ id: items.length + 1, name }); };
