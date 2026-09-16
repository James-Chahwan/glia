"""Prefix composition probe: FastAPI APIRouter(prefix=) + Flask Blueprint(url_prefix=).

Both frameworks declare the shared path segment on the *receiver*, not on the
decorator, so a parser that reads only the decorator's first string argument
emits `GET /{id}` / `GET /<int:id>` and loses the real path.
"""
from fastapi import APIRouter
from flask import Blueprint

router = APIRouter(prefix="/api/v1/users")


@router.get("/{id}")
async def get_user(id: int):
    return {"id": id}


bp = Blueprint("orders", __name__, url_prefix="/api/v1/orders")


@bp.route("/<int:id>", methods=["GET"])
def get_order(id):
    return {"id": id}
