"""User directory — one symbol referenced by the second Confluence fixture."""


def get_user(user_id):
    """Look up a user by id."""
    return {"id": user_id, "active": True}
