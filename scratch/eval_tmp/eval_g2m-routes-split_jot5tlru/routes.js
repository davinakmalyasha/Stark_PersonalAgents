const express = require('express');
const router = express.Router();
const items = [];
router.post('/api/items', (req, res) => { items.push({ id: items.length + 1, name: req.body.name }); res.status(201).json({ ok: true }); });
router.get('/api/items', (req, res) => res.json(items));
module.exports = router;
