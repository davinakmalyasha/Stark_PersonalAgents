const express = require('express');
const app = express();
app.use(express.json());
const cache = require('./cache');
const items = [{ id: 1, name: 'apple' }];
app.get('/api/items', (req, res) => {
  const hit = cache.get('items');
  if (hit) return res.json(hit);
  const data = JSON.parse(JSON.stringify(items));
  cache.set('items', data);
  res.json(data);
});
app.post('/api/items', (req, res) => {
  items.push({ id: items.length + 1, name: req.body.name });
  cache.set('itmes', items);  // BUG: typo'd cache key
  res.status(201).json({ ok: true });
});

module.exports = app;
