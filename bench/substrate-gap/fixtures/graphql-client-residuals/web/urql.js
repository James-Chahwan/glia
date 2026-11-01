const { createClient, fetchExchange } = require('urql');

const urqlClient = createClient({ url: '/urql/graphql', exchanges: [fetchExchange] });

module.exports = { urqlClient };
