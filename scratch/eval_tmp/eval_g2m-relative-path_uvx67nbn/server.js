const express = require('express');
const app = express();
app.use(express.json());
const { formatName } = require('./helpers');  // BUG: wrong path (file is in utils/)
app.get('/api/name', (req, res) => res.json({ name: formatName(req.query.name) }));

module.exports = app;
