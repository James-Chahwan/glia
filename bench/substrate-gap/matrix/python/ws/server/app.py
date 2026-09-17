from fastapi import FastAPI, WebSocket

app = FastAPI()


@app.websocket("/ws/chat")
async def chat(websocket: WebSocket):
    await websocket.accept()
    while True:
        text = await websocket.receive_text()
        await websocket.send_text(f"echo: {text}")


@app.websocket("/ws/admin")
async def admin(websocket: WebSocket):
    await websocket.accept()
    await websocket.close()
