import strawberry


@strawberry.type
class User:
    name: str


@strawberry.type
class Query:
    @strawberry.field
    def user(self, id: strawberry.ID) -> User:
        return User(name=f"user {id}")

    @strawberry.field
    def current_user(self) -> User:
        return User(name="me")


schema = strawberry.Schema(query=Query)
