import grpc
import websockets

import orders_pb2_grpc


async def follow():
    async with websockets.connect("ws://localhost:8765/live") as ws:
        return await ws.recv()


def orders_stub(channel):
    return orders_pb2_grpc.PaymentServiceStub(channel)
