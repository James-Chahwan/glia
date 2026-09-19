# strawberry code-first schema: the Python control.
import strawberry


@strawberry.type
class Recipe:
    title: str


@strawberry.type
class Query:
    @strawberry.field
    def recipe(self) -> Recipe:
        return Recipe(title="soup")
