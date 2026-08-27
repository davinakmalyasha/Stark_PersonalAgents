const express = require('express');
const app = express();
app.use(express.json());
const service = require('./service');
app.get('/api/items', (req, res) => res.json(service.list()));
app.post('/api/items', (req, res) => { service.create(req.body.name); res.status(201).json({ ok: true }); });

module.exports = app;
