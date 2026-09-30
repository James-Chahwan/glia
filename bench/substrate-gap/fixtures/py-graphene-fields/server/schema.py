import graphene


class User(graphene.ObjectType):
    id = graphene.ID()
    name = graphene.String()


class Query(graphene.ObjectType):
    all_users = graphene.List(User)
    user_by_id = graphene.Field(User, id=graphene.ID(required=True))

    def resolve_all_users(root, info):
        return []

    def resolve_user_by_id(root, info, id):
        return None


schema = graphene.Schema(query=Query)
