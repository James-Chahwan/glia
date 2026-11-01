from fastapi import APIRouter

from . import flows, users

router = APIRouter(prefix="/core")
router.include_router(users.router)
router.include_router(flows.router, prefix="/v1")
