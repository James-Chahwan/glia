module.exports = function charge(request) {
  return { transaction_id: 'tx-' + request.amount };
};
