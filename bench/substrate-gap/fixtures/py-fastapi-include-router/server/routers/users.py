from fastapi import APIRouter

router = APIRouter()


@router.get("/users")
def list_users():
    return []


@router.post("/users")
def create_user():
    return {}
