async def application(scope, receive, send):
    await send({"type": "http.response.start", "status": 200})
