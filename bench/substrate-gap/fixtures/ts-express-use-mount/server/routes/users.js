const express = require('express');

const router = express.Router();

router.get('/list', listUsers);
router.post('/', (req, res) => res.status(201).json(req.body));

function listUsers(req, res) {
  res.json([]);
}

module.exports = router;
