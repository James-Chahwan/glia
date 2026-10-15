import grpc

import demo_pb2
import demo_pb2_grpc


def recommend(target):
    channel = grpc.insecure_channel(target)
    stub = demo_pb2_grpc.CatalogServiceStub(channel)
    return stub.ListProducts(demo_pb2.Empty())
