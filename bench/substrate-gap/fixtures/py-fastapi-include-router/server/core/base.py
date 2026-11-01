from fastapi import APIRouter


class ServiceRouter(APIRouter):
    """A router subclass every service module builds its router from."""

    def __init__(self, prefix: str = "", **kwargs):
        super().__init__(prefix=prefix, **kwargs)
