from fastapi import FastAPI, WebSocket

app = FastAPI()


@app.websocket("/ws/chat")
async def chat(websocket: WebSocket):
    await websocket.accept()
    await websocket.send_text("hi")
