import 'package:graphql_flutter/graphql_flutter.dart';

final listShipments = gql(r'''
  query ListShipments { shipments { id } }
''');

Future<QueryResult> loadShipments(GraphQLClient client) {
  final options = QueryOptions(document: listShipments);
  return client.query(options);
}
