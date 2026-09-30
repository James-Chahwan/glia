import 'package:graphql/client.dart';

const listOrders = r"""
  query Orders {
    orders {
      id
    }
  }
""";

Future<QueryResult> fetchOrders(GraphQLClient client) {
  return client.query(QueryOptions(document: gql(listOrders)));
}
