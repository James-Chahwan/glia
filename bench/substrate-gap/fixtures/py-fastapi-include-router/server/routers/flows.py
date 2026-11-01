from core.base import ServiceRouter

router: ServiceRouter = ServiceRouter(prefix="/flows", tags=["Flows"])


@router.post("/filter")
def filter_flows():
    return []
