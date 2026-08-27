const express = require('express');
const app = express();
const router = require('./routes');
app.use(router);  // BUG: router mounted BEFORE json parser
app.use(express.json());
module.exports = app;
