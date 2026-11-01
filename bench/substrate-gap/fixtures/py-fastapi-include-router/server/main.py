from fastapi import FastAPI

from routers import router as core_router

app = FastAPI()
app.include_router(core_router, prefix="/svc")


@app.get("/health")
def health():
    return {"ok": True}
