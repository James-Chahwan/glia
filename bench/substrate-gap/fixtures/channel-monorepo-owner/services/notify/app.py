from fastapi import FastAPI, WebSocket

app = FastAPI()


@app.websocket("/ws")
async def notify_socket(websocket: WebSocket):
    await websocket.accept()
    while True:
        data = await websocket.receive_text()
        await websocket.send_text(data)
