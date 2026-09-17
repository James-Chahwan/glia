import 'package:shelf/shelf.dart';
import 'package:shelf_router/shelf_router.dart';

Response getUser(Request request, String id) => Response.ok('user $id');

Response createUser(Request request) => Response(201);

final router = Router()
  ..get('/users/<id>', getUser)
  ..post('/users', createUser);
